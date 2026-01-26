use crate::stream::{DesStreamSender, DesStreaming};
use crate::wire::{
    decode_stream_frame, decode_unary_response, encode_stream_frame, encode_unary_request,
    StreamDirection, StreamFrameKind, StreamFrameWire, UnaryRequestWire,
};
use crate::{stream, ClientResponseFuture};
use bytes::Bytes;
use des_components::transport::{
    EndpointId, MessageType, SharedEndpointRegistry, SimTransport, TransportEvent, TransportMessage,
};
use des_core::{defer_wake_after, scheduler::in_scheduler_context, Key, SchedulerHandle, SimTime};

use crate::util;
use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tonic::{
    metadata::{KeyAndValueRef, MetadataMap},
    Request, Response, Status,
};

use prost::Message;

pub type ClientStreamingResponseFuture =
    Pin<Box<dyn Future<Output = Result<Response<Bytes>, Status>> + 'static>>;

type PendingUnaryTx = des_tokio::sync::oneshot::Sender<Result<Response<Bytes>, Status>>;
type PendingUnaryMap = HashMap<String, PendingUnaryTx>;
type PendingUnary = Arc<Mutex<PendingUnaryMap>>;

/// Client-side channel for making unary RPC calls.
///
/// This is a simulation channel, not `tonic::transport::Channel`.
#[derive(Clone)]
pub struct Channel {
    service_name: String,
    transport_key: Key<TransportEvent>,
    endpoint_registry: SharedEndpointRegistry,
    scheduler: SchedulerHandle,

    client_endpoint: EndpointId,
    pending: PendingUnary,

    pending_streams: Arc<Mutex<HashMap<String, ClientStreamState>>>,

    next_counter: Arc<Mutex<u64>>,

    target_endpoint: Option<EndpointId>,
}

impl Channel {
    /// Create a `Channel` bound to a particular service name.
    pub(crate) fn new(
        service_name: String,
        transport_key: Key<TransportEvent>,
        endpoint_registry: SharedEndpointRegistry,
        scheduler: SchedulerHandle,
        client_endpoint: EndpointId,
        pending: PendingUnary,
        pending_streams: Arc<Mutex<HashMap<String, ClientStreamState>>>,
    ) -> Self {
        Self {
            service_name,
            transport_key,
            endpoint_registry,
            scheduler,
            client_endpoint,
            pending,
            pending_streams,
            next_counter: Arc::new(Mutex::new(0)),
            target_endpoint: None,
        }
    }

    pub fn client_endpoint(&self) -> EndpointId {
        self.client_endpoint
    }

    /// Convenience: connect to a specific server address (tonic-style).
    ///
    /// This is equivalent to `transport.connect(sim, service_name, addr)`.
    pub fn connect(
        sim: &mut des_core::Simulation,
        transport: &crate::Transport,
        service_name: impl Into<String>,
        addr: impl AsRef<str>,
    ) -> Result<Self, Status> {
        transport.connect(sim, service_name, addr)
    }

    /// Convenience: connect to a specific server `SocketAddr`.
    ///
    /// This is equivalent to `transport.connect_socket_addr(sim, service_name, addr)`.
    pub fn connect_socket_addr(
        sim: &mut des_core::Simulation,
        transport: &crate::Transport,
        service_name: impl Into<String>,
        addr: std::net::SocketAddr,
    ) -> Result<Self, Status> {
        transport.connect_socket_addr(sim, service_name, addr)
    }

    pub(crate) fn connect_addr(
        sim: &mut des_core::Simulation,
        transport: &crate::Transport,
        service_name: String,
        addr: std::net::SocketAddr,
        client_name: String,
    ) -> Result<Self, Status> {
        let instance_name = addr.to_string();
        let target_endpoint = EndpointId::new(format!("{service_name}:{instance_name}"));

        // Require an exact match (tonic-like: connect pins to a specific addr).
        let found = transport
            .endpoint_registry
            .find_endpoints(&service_name)
            .iter()
            .any(|e| e.id == target_endpoint);

        if !found {
            return Err(Status::unavailable(format!(
                "service not found at {addr} ({service_name})"
            )));
        }

        // Install a client endpoint component to receive responses.
        let endpoint_id = EndpointId::new(format!("client:{client_name}"));
        let client_endpoint = ClientEndpoint::new(endpoint_id);
        let pending = client_endpoint.pending.clone();
        let pending_streams = client_endpoint.pending_streams.clone();
        let client_key = sim.add_component(client_endpoint);

        {
            let transport_component = sim
                .get_component_mut::<TransportEvent, SimTransport>(transport.transport_key)
                .ok_or_else(|| Status::internal("transport component not found"))?;
            transport_component.register_handler(endpoint_id, client_key);
        }

        let mut channel = Channel::new(
            service_name,
            transport.transport_key,
            transport.endpoint_registry.clone(),
            transport.scheduler.clone(),
            endpoint_id,
            pending,
            pending_streams,
        );
        channel.target_endpoint = Some(target_endpoint);
        Ok(channel)
    }

    fn next_correlation_id(&self) -> String {
        let mut c = self.next_counter.lock().unwrap();
        *c += 1;
        format!("{}:{}:{}", self.service_name, self.client_endpoint.id(), *c)
    }

    /// Generate a deterministic stream identifier.
    ///
    /// Generate a deterministic stream identifier.
    fn next_stream_id(&self) -> String {
        let mut c = self.next_counter.lock().unwrap();
        *c += 1;
        format!(
            "{}:{}:stream:{}",
            self.service_name,
            self.client_endpoint.id(),
            *c
        )
    }

    fn schedule_transport(&self, delay: SimTime, event: TransportEvent) {
        if in_scheduler_context() {
            // Safe: does not lock the scheduler.
            defer_wake_after(delay, self.transport_key, event);
        } else {
            self.scheduler.schedule(delay, self.transport_key, event);
        }
    }

    fn send_rpc_stream_frame(&self, dst: EndpointId, stream_id: &str, frame: &StreamFrameWire) {
        let msg = TransportMessage::new(
            0,
            self.client_endpoint,
            dst,
            encode_stream_frame(frame).to_vec(),
            util::now(&self.scheduler),
            MessageType::RpcStreamFrame,
        )
        .with_correlation_id(stream_id.to_string());

        self.schedule_transport(
            SimTime::zero(),
            TransportEvent::SendMessage { message: msg },
        );
    }

    async fn select_target(
        &self,
        timeout: Option<Duration>,
    ) -> Result<des_components::transport::EndpointInfo, Status> {
        let target = if let Some(target) = self.target_endpoint {
            // Validate the server endpoint is still registered.
            let ok = self
                .endpoint_registry
                .find_endpoints(&self.service_name)
                .iter()
                .any(|e| e.id == target);

            if !ok {
                return Err(Status::unavailable(format!(
                    "service not found at fixed address: {}",
                    self.service_name
                )));
            }

            des_components::transport::EndpointInfo {
                id: target,
                service_name: self.service_name.clone(),
                instance_name: String::new(),
                metadata: std::collections::HashMap::new(),
            }
        } else {
            let fut = des_tower::wait_for_endpoint(
                self.endpoint_registry.clone(),
                self.service_name.clone(),
                None,
            );

            let ep = match timeout {
                None => fut.await,
                Some(t) => match des_tokio::time::timeout(t, fut).await {
                    Ok(r) => r,
                    Err(_) => {
                        return Err(Status::deadline_exceeded(
                            "request timed out while waiting for endpoint",
                        ));
                    }
                },
            };

            match ep {
                Some(ep) => ep,
                None => {
                    return Err(Status::deadline_exceeded(
                        "request timed out while waiting for endpoint",
                    ))
                }
            }
        };

        Ok(target)
    }

    /// Make a unary RPC call.
    pub async fn unary(
        &self,
        path: &str,
        request: Request<Bytes>,
        timeout: Option<Duration>,
    ) -> Result<Response<Bytes>, Status> {
        // Tonic's dynamic balancers effectively include endpoint discovery in the overall
        // request timeout. We mimic that by wrapping the full unary path (discovery + send
        // + response) in a single timeout.
        let Some(timeout) = timeout else {
            return self.unary_inner(path, request).await;
        };

        match des_tokio::time::timeout(timeout, self.unary_inner(path, request)).await {
            Ok(r) => r,
            Err(_) => Err(Status::deadline_exceeded("request timed out")),
        }
    }

    pub async fn unary_prost<Req, Resp>(
        &self,
        path: &'static str,
        request: Request<Req>,
        timeout: Option<Duration>,
    ) -> Result<Response<Resp>, Status>
    where
        Req: Message,
        Resp: Message + Default,
    {
        let (metadata, extensions, msg) = request.into_parts();
        let req_bytes = Request::from_parts(metadata, extensions, Bytes::from(msg.encode_to_vec()));

        let resp = self.unary(path, req_bytes, timeout).await?;
        let (metadata, payload, extensions) = resp.into_parts();

        let decoded = Resp::decode(payload)
            .map_err(|e| Status::internal(format!("prost decode error: {e}")))?;
        Ok(Response::from_parts(metadata, decoded, extensions))
    }

    /// Open a client-streaming RPC.
    ///
    /// The returned sender is used to stream request messages to the server.
    /// Dropping all sender clones completes the request stream.
    ///
    /// The returned future resolves to the server's single response.
    pub async fn client_streaming(
        &self,
        path: &str,
        timeout: Option<Duration>,
    ) -> Result<(DesStreamSender<Bytes>, ClientStreamingResponseFuture), Status> {
        let target = self.select_target(timeout).await?;

        let stream_id = self.next_stream_id();

        let (request_tx, mut request_rx) = des_tokio::sync::mpsc::channel::<Bytes>(16);
        let sender = DesStreamSender::new(request_tx);

        let (response_tx, response_rx) = des_tokio::sync::oneshot::channel();

        {
            let mut pending = self.pending_streams.lock().unwrap();
            pending.insert(
                stream_id.clone(),
                ClientStreamState {
                    mode: ClientStreamMode::Client,
                    next_expected_s2c: 0,
                    saw_s2c_open: false,
                    reorder: BTreeMap::new(),
                    response_tx: Some(response_tx),
                    response_payload: None,
                    stream_tx: None,
                },
            );
        }

        // Send Open.
        let open = StreamFrameWire {
            stream_id: stream_id.clone(),
            direction: StreamDirection::ClientToServer,
            seq: 0,
            kind: StreamFrameKind::Open,
            method: Some(path.to_string()),
            metadata: Vec::new(),
            payload: Bytes::new(),
            status_ok: true,
            status_code: tonic::Code::Ok,
            status_message: String::new(),
        };

        self.send_rpc_stream_frame(target.id, &stream_id, &open);

        // Forward request items.
        let channel = self.clone();
        let stream_id_for_task = stream_id.clone();
        let target_id_for_task = target.id;
        des_tokio::task::spawn_local(async move {
            let mut seq = 1u64;
            while let Some(payload) = request_rx.recv().await {
                let frame = StreamFrameWire {
                    stream_id: stream_id_for_task.clone(),
                    direction: StreamDirection::ClientToServer,
                    seq,
                    kind: StreamFrameKind::Data,
                    method: None,
                    metadata: Vec::new(),
                    payload,
                    status_ok: true,
                    status_code: tonic::Code::Ok,
                    status_message: String::new(),
                };

                channel.send_rpc_stream_frame(target_id_for_task, &stream_id_for_task, &frame);
                seq += 1;
            }

            let close = StreamFrameWire {
                stream_id: stream_id_for_task.clone(),
                direction: StreamDirection::ClientToServer,
                seq,
                kind: StreamFrameKind::Close,
                method: None,
                metadata: Vec::new(),
                payload: Bytes::new(),
                status_ok: true,
                status_code: tonic::Code::Ok,
                status_message: String::new(),
            };

            channel.send_rpc_stream_frame(target_id_for_task, &stream_id_for_task, &close);
        });

        let channel = self.clone();
        let stream_id_for_response = stream_id.clone();
        let response_future: ClientStreamingResponseFuture = Box::pin(async move {
            let rx_result = match timeout {
                None => response_rx
                    .await
                    .map_err(|_| Status::cancelled("stream cancelled")),
                Some(t) => match des_tokio::time::timeout(t, response_rx).await {
                    Ok(r) => r.map_err(|_| Status::cancelled("stream cancelled")),
                    Err(_) => {
                        {
                            let mut pending = channel.pending_streams.lock().unwrap();
                            pending.remove(&stream_id_for_response);
                        }

                        // Best-effort cancel.
                        let cancel = StreamFrameWire {
                            stream_id: stream_id_for_response.clone(),
                            direction: StreamDirection::ClientToServer,
                            seq: 0,
                            kind: StreamFrameKind::Cancel,
                            method: None,
                            metadata: Vec::new(),
                            payload: Bytes::new(),
                            status_ok: false,
                            status_code: tonic::Code::Cancelled,
                            status_message: "deadline exceeded".to_string(),
                        };
                        channel.send_rpc_stream_frame(
                            target_id_for_task,
                            &stream_id_for_response,
                            &cancel,
                        );

                        return Err(Status::deadline_exceeded("stream response timed out"));
                    }
                },
            };

            match rx_result {
                Ok(r) => r,
                Err(e) => Err(e),
            }
        });

        Ok((sender, response_future))
    }

    pub async fn client_streaming_prost<Req, Resp>(
        &self,
        path: &'static str,
        timeout: Option<Duration>,
    ) -> Result<(stream::Sender<Req>, ClientResponseFuture<Resp>), Status>
    where
        Req: Message + 'static,
        Resp: Message + Default,
    {
        let (bytes_sender, resp_fut) = self.client_streaming(path, timeout).await?;
        let (sender, mut typed_in) = stream::channel::<Req>(16);

        des_tokio::task::spawn_local(async move {
            while let Some(item) = typed_in.next().await {
                match item {
                    Ok(msg) => {
                        let _ = bytes_sender.send(Bytes::from(msg.encode_to_vec())).await;
                    }
                    Err(_status) => {
                        // There is no request-stream error signaling on the wire yet.
                        break;
                    }
                }
            }
            bytes_sender.close();
        });

        let typed_fut: ClientResponseFuture<Resp> = Box::pin(async move {
            let resp = resp_fut.await?;
            let (metadata, payload, extensions) = resp.into_parts();
            let decoded = Resp::decode(payload)
                .map_err(|e| Status::internal(format!("prost decode error: {e}")))?;
            Ok(Response::from_parts(metadata, decoded, extensions))
        });

        Ok((sender, typed_fut))
    }

    /// Open a server-streaming RPC.
    ///
    /// Sends a single request message and returns a stream of response messages.
    pub async fn server_streaming(
        &self,
        path: &str,
        request: Request<Bytes>,
        timeout: Option<Duration>,
    ) -> Result<Response<DesStreaming<Bytes>>, Status> {
        let target = self.select_target(timeout).await?;

        let stream_id = self.next_stream_id();
        let (tx, rx) = des_tokio::sync::mpsc::channel::<Result<Bytes, Status>>(16);

        {
            let mut pending = self.pending_streams.lock().unwrap();
            pending.insert(
                stream_id.clone(),
                ClientStreamState {
                    mode: ClientStreamMode::Server,
                    next_expected_s2c: 0,
                    saw_s2c_open: false,
                    reorder: BTreeMap::new(),
                    response_tx: None,
                    response_payload: None,
                    stream_tx: Some(tx),
                },
            );
        }

        // Send Open.
        let open = StreamFrameWire {
            stream_id: stream_id.clone(),
            direction: StreamDirection::ClientToServer,
            seq: 0,
            kind: StreamFrameKind::Open,
            method: Some(path.to_string()),
            metadata: Vec::new(),
            payload: Bytes::new(),
            status_ok: true,
            status_code: tonic::Code::Ok,
            status_message: String::new(),
        };

        self.send_rpc_stream_frame(target.id, &stream_id, &open);

        // Send request payload as Data(seq=1) and then Close(seq=2).
        let req_payload = request.into_inner();

        let data = StreamFrameWire {
            stream_id: stream_id.clone(),
            direction: StreamDirection::ClientToServer,
            seq: 1,
            kind: StreamFrameKind::Data,
            method: None,
            metadata: Vec::new(),
            payload: req_payload,
            status_ok: true,
            status_code: tonic::Code::Ok,
            status_message: String::new(),
        };

        self.send_rpc_stream_frame(target.id, &stream_id, &data);

        let close = StreamFrameWire {
            stream_id: stream_id.clone(),
            direction: StreamDirection::ClientToServer,
            seq: 2,
            kind: StreamFrameKind::Close,
            method: None,
            metadata: Vec::new(),
            payload: Bytes::new(),
            status_ok: true,
            status_code: tonic::Code::Ok,
            status_message: String::new(),
        };

        self.send_rpc_stream_frame(target.id, &stream_id, &close);

        // Optional overall timeout: best-effort cancellation and local termination.
        if let Some(timeout) = timeout {
            let channel = self.clone();
            let stream_id_for_timer = stream_id.clone();
            let target_id = target.id;
            des_tokio::task::spawn_local(async move {
                des_tokio::time::sleep(timeout).await;

                let tx = {
                    let mut pending = channel.pending_streams.lock().unwrap();
                    let Some(mut state) = pending.remove(&stream_id_for_timer) else {
                        return;
                    };
                    state.stream_tx.take()
                };

                if let Some(tx) = tx {
                    let _ = tx
                        .send(Err(Status::deadline_exceeded("stream timed out")))
                        .await;
                }

                // Best-effort cancel.
                let cancel = StreamFrameWire {
                    stream_id: stream_id_for_timer.clone(),
                    direction: StreamDirection::ClientToServer,
                    seq: 0,
                    kind: StreamFrameKind::Cancel,
                    method: None,
                    metadata: Vec::new(),
                    payload: Bytes::new(),
                    status_ok: false,
                    status_code: tonic::Code::Cancelled,
                    status_message: "deadline exceeded".to_string(),
                };

                let msg = TransportMessage::new(
                    0,
                    channel.client_endpoint,
                    target_id,
                    encode_stream_frame(&cancel).to_vec(),
                    util::now(&channel.scheduler),
                    MessageType::RpcStreamFrame,
                )
                .with_correlation_id(stream_id_for_timer);

                channel.schedule_transport(
                    SimTime::zero(),
                    TransportEvent::SendMessage { message: msg },
                );
            });
        }

        Ok(Response::new(DesStreaming::new(rx)))
    }

    pub async fn server_streaming_prost<Req, Resp>(
        &self,
        path: &'static str,
        request: Request<Req>,
        timeout: Option<Duration>,
    ) -> Result<Response<DesStreaming<Resp>>, Status>
    where
        Req: Message,
        Resp: Message + Default + 'static,
    {
        let (metadata, extensions, msg) = request.into_parts();
        let req_bytes = Request::from_parts(metadata, extensions, Bytes::from(msg.encode_to_vec()));
        let resp = self.server_streaming(path, req_bytes, timeout).await?;
        let (metadata, mut stream_bytes, extensions) = resp.into_parts();

        let (tx, stream_typed) = stream::channel::<Resp>(16);
        des_tokio::task::spawn_local(async move {
            while let Some(item) = stream_bytes.next().await {
                match item {
                    Ok(bytes) => match Resp::decode(bytes) {
                        Ok(msg) => {
                            if tx.send(msg).await.is_err() {
                                break;
                            }
                        }
                        Err(e) => {
                            let _ = tx
                                .send_err(Status::internal(format!("prost decode error: {e}")))
                                .await;
                            break;
                        }
                    },
                    Err(status) => {
                        let _ = tx.send_err(status).await;
                        break;
                    }
                }
            }
            tx.close();
        });

        Ok(Response::from_parts(metadata, stream_typed, extensions))
    }

    /// Open a bidirectional-streaming RPC.
    ///
    /// Returns a sender for outbound request messages and a response stream.
    pub async fn bidirectional_streaming(
        &self,
        path: &str,
        timeout: Option<Duration>,
    ) -> Result<(DesStreamSender<Bytes>, Response<DesStreaming<Bytes>>), Status> {
        let target = self.select_target(timeout).await?;

        let stream_id = self.next_stream_id();

        let (request_tx, mut request_rx) = des_tokio::sync::mpsc::channel::<Bytes>(16);
        let sender = DesStreamSender::new(request_tx);

        let (stream_tx, stream_rx) = des_tokio::sync::mpsc::channel::<Result<Bytes, Status>>(16);

        {
            let mut pending = self.pending_streams.lock().unwrap();
            pending.insert(
                stream_id.clone(),
                ClientStreamState {
                    mode: ClientStreamMode::Bidi,
                    next_expected_s2c: 0,
                    saw_s2c_open: false,
                    reorder: BTreeMap::new(),
                    response_tx: None,
                    response_payload: None,
                    stream_tx: Some(stream_tx),
                },
            );
        }

        // Send Open.
        let open = StreamFrameWire {
            stream_id: stream_id.clone(),
            direction: StreamDirection::ClientToServer,
            seq: 0,
            kind: StreamFrameKind::Open,
            method: Some(path.to_string()),
            metadata: Vec::new(),
            payload: Bytes::new(),
            status_ok: true,
            status_code: tonic::Code::Ok,
            status_message: String::new(),
        };

        self.send_rpc_stream_frame(target.id, &stream_id, &open);

        // Forward request items.
        let channel = self.clone();
        let stream_id_for_task = stream_id.clone();
        let target_id_for_task = target.id;
        des_tokio::task::spawn_local(async move {
            let mut seq = 1u64;
            while let Some(payload) = request_rx.recv().await {
                let frame = StreamFrameWire {
                    stream_id: stream_id_for_task.clone(),
                    direction: StreamDirection::ClientToServer,
                    seq,
                    kind: StreamFrameKind::Data,
                    method: None,
                    metadata: Vec::new(),
                    payload,
                    status_ok: true,
                    status_code: tonic::Code::Ok,
                    status_message: String::new(),
                };

                channel.send_rpc_stream_frame(target_id_for_task, &stream_id_for_task, &frame);

                seq += 1;
            }

            let close = StreamFrameWire {
                stream_id: stream_id_for_task.clone(),
                direction: StreamDirection::ClientToServer,
                seq,
                kind: StreamFrameKind::Close,
                method: None,
                metadata: Vec::new(),
                payload: Bytes::new(),
                status_ok: true,
                status_code: tonic::Code::Ok,
                status_message: String::new(),
            };

            channel.send_rpc_stream_frame(target_id_for_task, &stream_id_for_task, &close);
        });

        // Optional overall timeout: best-effort cancellation and local termination.
        if let Some(timeout) = timeout {
            let channel = self.clone();
            let stream_id_for_timer = stream_id.clone();
            let target_id = target.id;
            des_tokio::task::spawn_local(async move {
                des_tokio::time::sleep(timeout).await;

                let tx = {
                    let mut pending = channel.pending_streams.lock().unwrap();
                    let Some(mut state) = pending.remove(&stream_id_for_timer) else {
                        return;
                    };
                    state.stream_tx.take()
                };

                if let Some(tx) = tx {
                    let _ = tx
                        .send(Err(Status::deadline_exceeded("stream timed out")))
                        .await;
                }

                let cancel = StreamFrameWire {
                    stream_id: stream_id_for_timer.clone(),
                    direction: StreamDirection::ClientToServer,
                    seq: 0,
                    kind: StreamFrameKind::Cancel,
                    method: None,
                    metadata: Vec::new(),
                    payload: Bytes::new(),
                    status_ok: false,
                    status_code: tonic::Code::Cancelled,
                    status_message: "deadline exceeded".to_string(),
                };

                channel.send_rpc_stream_frame(target_id, &stream_id_for_timer, &cancel);
            });
        }

        Ok((sender, Response::new(DesStreaming::new(stream_rx))))
    }

    pub async fn bidirectional_streaming_prost<Req, Resp>(
        &self,
        path: &'static str,
        timeout: Option<Duration>,
    ) -> Result<(stream::Sender<Req>, Response<DesStreaming<Resp>>), Status>
    where
        Req: Message + 'static,
        Resp: Message + Default + 'static,
    {
        let (bytes_sender, resp) = self.bidirectional_streaming(path, timeout).await?;
        let (sender, mut typed_in) = stream::channel::<Req>(16);

        des_tokio::task::spawn_local(async move {
            while let Some(item) = typed_in.next().await {
                match item {
                    Ok(msg) => {
                        let _ = bytes_sender.send(Bytes::from(msg.encode_to_vec())).await;
                    }
                    Err(_status) => {
                        // There is no request-stream error signaling on the wire yet.
                        break;
                    }
                }
            }
            bytes_sender.close();
        });

        let (metadata, mut stream_bytes, extensions) = resp.into_parts();
        let (tx, stream_typed) = stream::channel::<Resp>(16);
        des_tokio::task::spawn_local(async move {
            while let Some(item) = stream_bytes.next().await {
                match item {
                    Ok(bytes) => match Resp::decode(bytes) {
                        Ok(msg) => {
                            if tx.send(msg).await.is_err() {
                                break;
                            }
                        }
                        Err(e) => {
                            let _ = tx
                                .send_err(Status::internal(format!("prost decode error: {e}")))
                                .await;
                            break;
                        }
                    },
                    Err(status) => {
                        let _ = tx.send_err(status).await;
                        break;
                    }
                }
            }
            tx.close();
        });

        Ok((
            sender,
            Response::from_parts(metadata, stream_typed, extensions),
        ))
    }

    async fn unary_inner(
        &self,
        path: &str,
        request: Request<Bytes>,
    ) -> Result<Response<Bytes>, Status> {
        // Unary timeout is handled by the caller by wrapping `unary_inner`, so
        // endpoint selection here should not apply an additional timeout.
        let target = self.select_target(None).await?;

        let correlation_id = self.next_correlation_id();

        // Convert metadata to owned strings.
        let metadata: Vec<(String, String)> = request
            .metadata()
            .iter()
            .filter_map(|kv| match kv {
                KeyAndValueRef::Ascii(k, v) => {
                    Some((k.as_str().to_string(), v.to_str().ok()?.to_string()))
                }
                _ => None,
            })
            .collect();

        let msg = UnaryRequestWire {
            method: path.to_string(),
            metadata,
            payload: request.get_ref().clone(),
        };

        let payload = encode_unary_request(&msg);

        // Register pending response.
        let (tx, rx) = des_tokio::sync::oneshot::channel();
        {
            let mut pending = self.pending.lock().unwrap();
            pending.insert(correlation_id.clone(), tx);
        }

        let message = TransportMessage::new(
            0,
            self.client_endpoint,
            target.id,
            payload.to_vec(),
            util::now(&self.scheduler),
            MessageType::UnaryRequest,
        )
        .with_correlation_id(correlation_id.clone());

        self.schedule_transport(SimTime::zero(), TransportEvent::SendMessage { message });

        // The `timeout` has already been accounted for (endpoint wait + response wait).
        let result = rx.await;

        match result {
            Ok(Ok(resp)) => Ok(resp),
            Ok(Err(status)) => Err(status),
            Err(_) => {
                // Receiver dropped: request cancelled (e.g. client task aborted).
                let mut pending = self.pending.lock().unwrap();
                pending.remove(&correlation_id);
                Err(Status::cancelled("request cancelled"))
            }
        }
    }
}

/// Installed client artifacts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClientStreamMode {
    Client,
    Server,
    Bidi,
}

pub(crate) struct ClientStreamState {
    mode: ClientStreamMode,

    /// Next expected sequence number for S->C frames.
    next_expected_s2c: u64,

    /// Whether we've observed an explicit or implicit S->C open.
    saw_s2c_open: bool,

    reorder: BTreeMap<u64, StreamFrameWire>,

    // --- client-streaming response ---
    response_tx: Option<des_tokio::sync::oneshot::Sender<Result<Response<Bytes>, Status>>>,
    response_payload: Option<Bytes>,

    // --- server-streaming response ---
    stream_tx: Option<des_tokio::sync::mpsc::Sender<Result<Bytes, Status>>>,
}

pub struct InstalledClient {
    pub endpoint_id: EndpointId,
    pub key: Key<TransportEvent>,
    pub channel: Channel,
}

pub struct ClientBuilder {
    service_name: String,
    transport_key: Key<TransportEvent>,
    endpoint_registry: SharedEndpointRegistry,
    scheduler: SchedulerHandle,
    client_name: String,
    endpoint_id: Option<EndpointId>,
}

impl ClientBuilder {
    pub fn new(
        service_name: String,
        transport_key: Key<TransportEvent>,
        endpoint_registry: SharedEndpointRegistry,
        scheduler: SchedulerHandle,
    ) -> Self {
        Self {
            service_name,
            transport_key,
            endpoint_registry,
            scheduler,
            client_name: "client".to_string(),
            endpoint_id: None,
        }
    }

    pub fn client_name(mut self, name: impl Into<String>) -> Self {
        self.client_name = name.into();
        self
    }

    pub fn endpoint_id(mut self, endpoint_id: EndpointId) -> Self {
        self.endpoint_id = Some(endpoint_id);
        self
    }

    pub fn install(self, sim: &mut des_core::Simulation) -> Result<InstalledClient, Status> {
        let endpoint_id = self
            .endpoint_id
            .unwrap_or_else(|| EndpointId::new(format!("client:{}", self.client_name)));

        let client_endpoint = ClientEndpoint::new(endpoint_id);
        let pending = client_endpoint.pending.clone();
        let pending_streams = client_endpoint.pending_streams.clone();
        let client_key = sim.add_component(client_endpoint);

        {
            let transport = sim
                .get_component_mut::<TransportEvent, SimTransport>(self.transport_key)
                .ok_or_else(|| Status::internal("transport component not found"))?;
            transport.register_handler(endpoint_id, client_key);
        }

        let channel = Channel::new(
            self.service_name,
            self.transport_key,
            self.endpoint_registry,
            self.scheduler,
            endpoint_id,
            pending,
            pending_streams,
        );

        Ok(InstalledClient {
            endpoint_id,
            key: client_key,
            channel,
        })
    }
}

/// Client endpoint that receives transport events and resolves pending RPC futures.
pub struct ClientEndpoint {
    pub endpoint_id: EndpointId,
    pub pending: PendingUnary,
    pending_streams: Arc<Mutex<HashMap<String, ClientStreamState>>>,
}

impl ClientEndpoint {
    pub fn new(endpoint_id: EndpointId) -> Self {
        Self {
            endpoint_id,
            pending: Arc::new(Mutex::new(HashMap::new())),
            pending_streams: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl des_core::Component for ClientEndpoint {
    type Event = TransportEvent;

    #[allow(clippy::single_match)]
    fn process_event(
        &mut self,
        _self_id: Key<Self::Event>,
        event: &Self::Event,
        _scheduler: &mut des_core::Scheduler,
    ) {
        match event {
            TransportEvent::MessageDelivered { message } => match message.message_type {
                MessageType::UnaryResponse => {
                    let correlation_id = match &message.correlation_id {
                        Some(c) => c.clone(),
                        None => return,
                    };

                    let bytes = Bytes::from(message.payload.clone());
                    let decoded = decode_unary_response(bytes);

                    let mut pending = self.pending.lock().unwrap();
                    if let Some(tx) = pending.remove(&correlation_id) {
                        let _ =
                            match decoded {
                                Ok(wire) => {
                                    if wire.ok {
                                        let mut resp = Response::new(wire.payload);
                                        let meta: &mut MetadataMap = resp.metadata_mut();
                                        for (k, v) in wire.metadata {
                                            if let (Ok(k), Ok(v)) = (
                                                k.parse::<tonic::metadata::MetadataKey<
                                                    tonic::metadata::Ascii,
                                                >>(
                                                ),
                                                v.parse::<tonic::metadata::MetadataValue<
                                                    tonic::metadata::Ascii,
                                                >>(
                                                ),
                                            ) {
                                                meta.insert(k, v);
                                            }
                                        }
                                        tx.send(Ok(resp))
                                    } else {
                                        let status = Status::new(wire.code, wire.message);
                                        // (metadata ignored for error for now)
                                        tx.send(Err(status))
                                    }
                                }
                                Err(status) => tx.send(Err(status)),
                            };
                    }
                }
                MessageType::RpcStreamFrame => {
                    const MAX_REORDER_FRAMES: usize = 64;

                    let stream_id = match &message.correlation_id {
                        Some(c) => c.clone(),
                        None => return,
                    };

                    let bytes = Bytes::from(message.payload.clone());
                    let mut frame = match decode_stream_frame(bytes) {
                        Ok(f) => f,
                        Err(status) => {
                            // Fail-fast: resolve any pending waiter and cleanup.
                            let mut pending = self.pending_streams.lock().unwrap();
                            if let Some(mut state) = pending.remove(&stream_id) {
                                match state.mode {
                                    ClientStreamMode::Client => {
                                        if let Some(tx) = state.response_tx.take() {
                                            let _ = tx.send(Err(status));
                                        }
                                    }
                                    ClientStreamMode::Server | ClientStreamMode::Bidi => {
                                        if let Some(tx) = state.stream_tx.take() {
                                            let _ = tx.try_send(Err(status));
                                        }
                                    }
                                }
                            }
                            return;
                        }
                    };

                    // `TransportMessage.correlation_id` is authoritative for routing.
                    if frame.stream_id != stream_id {
                        frame.stream_id = stream_id.clone();
                    }

                    if frame.direction != StreamDirection::ServerToClient {
                        return;
                    }

                    let mut pending = self.pending_streams.lock().unwrap();
                    let Some(state) = pending.get_mut(&stream_id) else {
                        return;
                    };

                    if state.reorder.len() >= MAX_REORDER_FRAMES {
                        match state.mode {
                            ClientStreamMode::Client => {
                                if let Some(tx) = state.response_tx.take() {
                                    let _ = tx.send(Err(Status::resource_exhausted(
                                        "stream reorder buffer exceeded",
                                    )));
                                }
                            }
                            ClientStreamMode::Server | ClientStreamMode::Bidi => {
                                if let Some(tx) = state.stream_tx.take() {
                                    let _ = tx.try_send(Err(Status::resource_exhausted(
                                        "stream reorder buffer exceeded",
                                    )));
                                }
                            }
                        }
                        pending.remove(&stream_id);
                        return;
                    }

                    state.reorder.insert(frame.seq, frame);

                    // If the peer doesn't send an explicit S->C Open (seq=0), accept a
                    // "Data/Close begins at seq=1" convention to avoid deadlocks.
                    if !state.saw_s2c_open
                        && state.next_expected_s2c == 0
                        && !state.reorder.contains_key(&0)
                        && state.reorder.contains_key(&1)
                    {
                        state.saw_s2c_open = true;
                        state.next_expected_s2c = 1;
                    }

                    while let Some(next) = state.reorder.remove(&state.next_expected_s2c) {
                        state.next_expected_s2c += 1;

                        match next.kind {
                            StreamFrameKind::Open => {
                                state.saw_s2c_open = true;
                            }
                            StreamFrameKind::Data => match state.mode {
                                ClientStreamMode::Client => {
                                    if !state.saw_s2c_open {
                                        if let Some(tx) = state.response_tx.take() {
                                            let _ = tx.send(Err(Status::internal(
                                                "stream protocol violation: data before open",
                                            )));
                                        }
                                        pending.remove(&stream_id);
                                        break;
                                    }

                                    if state.response_payload.is_some() {
                                        if let Some(tx) = state.response_tx.take() {
                                            let _ = tx.send(Err(Status::internal(
                                                "stream protocol violation: multiple response data frames",
                                            )));
                                        }
                                        pending.remove(&stream_id);
                                        break;
                                    }

                                    state.response_payload = Some(next.payload);
                                }
                                ClientStreamMode::Server | ClientStreamMode::Bidi => {
                                    if !state.saw_s2c_open {
                                        if let Some(tx) = state.stream_tx.take() {
                                            let _ = tx.try_send(Err(Status::internal(
                                                "stream protocol violation: data before open",
                                            )));
                                        }
                                        pending.remove(&stream_id);
                                        break;
                                    }

                                    if let Some(tx) = &state.stream_tx {
                                        if tx.try_send(Ok(next.payload)).is_err() {
                                            // Backpressure: terminate the stream.
                                            pending.remove(&stream_id);
                                            break;
                                        }
                                    }
                                }
                            },
                            StreamFrameKind::Close => {
                                match state.mode {
                                    ClientStreamMode::Client => {
                                        if let Some(tx) = state.response_tx.take() {
                                            if next.status_ok {
                                                let payload = state
                                                    .response_payload
                                                    .take()
                                                    .unwrap_or_else(Bytes::new);
                                                let resp = Response::new(payload);
                                                let _ = tx.send(Ok(resp));
                                            } else {
                                                let status = Status::new(
                                                    next.status_code,
                                                    next.status_message,
                                                );
                                                let _ = tx.send(Err(status));
                                            }
                                        }
                                    }
                                    ClientStreamMode::Server | ClientStreamMode::Bidi => {
                                        if next.status_ok {
                                            state.stream_tx.take();
                                        } else if let Some(tx) = state.stream_tx.take() {
                                            let status =
                                                Status::new(next.status_code, next.status_message);
                                            let _ = tx.try_send(Err(status));
                                        }
                                    }
                                }

                                pending.remove(&stream_id);
                                break;
                            }
                            StreamFrameKind::Cancel => {
                                match state.mode {
                                    ClientStreamMode::Client => {
                                        if let Some(tx) = state.response_tx.take() {
                                            let _ =
                                                tx.send(Err(Status::cancelled("stream cancelled")));
                                        }
                                    }
                                    ClientStreamMode::Server | ClientStreamMode::Bidi => {
                                        if let Some(tx) = state.stream_tx.take() {
                                            let _ = tx.try_send(Err(Status::cancelled(
                                                "stream cancelled",
                                            )));
                                        }
                                    }
                                }

                                pending.remove(&stream_id);
                                break;
                            }
                        }
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }
}

#[cfg(test)]
mod client_streaming_tests {
    use super::*;
    use des_components::transport::{EndpointInfo, SimTransport};
    use des_core::{
        Component, Execute, Executor, Scheduler, SimTime, Simulation, SimulationConfig,
    };

    struct TestClientStreamingServer {
        endpoint_id: EndpointId,
        transport_key: Key<TransportEvent>,
    }

    struct TestClientStreamingServerNoOpen {
        endpoint_id: EndpointId,
        transport_key: Key<TransportEvent>,
    }

    impl Component for TestClientStreamingServer {
        type Event = TransportEvent;

        fn process_event(
            &mut self,
            _self_id: Key<Self::Event>,
            event: &Self::Event,
            scheduler: &mut Scheduler,
        ) {
            let TransportEvent::MessageDelivered { message } = event else {
                return;
            };

            if message.message_type != MessageType::RpcStreamFrame {
                return;
            }

            let stream_id = match &message.correlation_id {
                Some(c) => c.clone(),
                None => return,
            };

            let frame = match decode_stream_frame(Bytes::from(message.payload.clone())) {
                Ok(f) => f,
                Err(_) => return,
            };

            if frame.stream_id != stream_id {
                return;
            }

            // Respond when the client closes: return a single Data frame and a Close.
            if frame.direction == StreamDirection::ClientToServer
                && frame.kind == StreamFrameKind::Close
            {
                let response_data = StreamFrameWire {
                    stream_id: stream_id.clone(),
                    direction: StreamDirection::ServerToClient,
                    seq: 1,
                    kind: StreamFrameKind::Data,
                    method: None,
                    metadata: Vec::new(),
                    payload: Bytes::from_static(b"ok"),
                    status_ok: true,
                    status_code: tonic::Code::Ok,
                    status_message: String::new(),
                };

                let response_close = StreamFrameWire {
                    stream_id: stream_id.clone(),
                    direction: StreamDirection::ServerToClient,
                    seq: 2,
                    kind: StreamFrameKind::Close,
                    method: None,
                    metadata: Vec::new(),
                    payload: Bytes::new(),
                    status_ok: true,
                    status_code: tonic::Code::Ok,
                    status_message: String::new(),
                };

                for frame in [
                    StreamFrameWire {
                        // Open (seq=0) is optional for this minimal test, but helps enforce ordering.
                        stream_id: stream_id.clone(),
                        direction: StreamDirection::ServerToClient,
                        seq: 0,
                        kind: StreamFrameKind::Open,
                        method: Some("/svc.Test/ClientStreaming".to_string()),
                        metadata: Vec::new(),
                        payload: Bytes::new(),
                        status_ok: true,
                        status_code: tonic::Code::Ok,
                        status_message: String::new(),
                    },
                    response_data,
                    response_close,
                ] {
                    let bytes = encode_stream_frame(&frame);
                    let msg = TransportMessage::new(
                        0,
                        self.endpoint_id,
                        message.source,
                        bytes.to_vec(),
                        scheduler.time(),
                        MessageType::RpcStreamFrame,
                    )
                    .with_correlation_id(stream_id.clone());

                    scheduler.schedule(
                        SimTime::zero(),
                        self.transport_key,
                        TransportEvent::SendMessage { message: msg },
                    );
                }
            }
        }
    }

    impl Component for TestClientStreamingServerNoOpen {
        type Event = TransportEvent;

        fn process_event(
            &mut self,
            _self_id: Key<Self::Event>,
            event: &Self::Event,
            scheduler: &mut Scheduler,
        ) {
            let TransportEvent::MessageDelivered { message } = event else {
                return;
            };

            if message.message_type != MessageType::RpcStreamFrame {
                return;
            }

            let stream_id = match &message.correlation_id {
                Some(c) => c.clone(),
                None => return,
            };

            let frame = match decode_stream_frame(Bytes::from(message.payload.clone())) {
                Ok(f) => f,
                Err(_) => return,
            };

            if frame.stream_id != stream_id {
                return;
            }

            // Respond when the client closes: return a single Data frame and a Close.
            // Intentionally omit Open to exercise the client's implicit-open fallback.
            if frame.direction == StreamDirection::ClientToServer
                && frame.kind == StreamFrameKind::Close
            {
                for frame in [
                    StreamFrameWire {
                        stream_id: stream_id.clone(),
                        direction: StreamDirection::ServerToClient,
                        seq: 1,
                        kind: StreamFrameKind::Data,
                        method: None,
                        metadata: Vec::new(),
                        payload: Bytes::from_static(b"ok"),
                        status_ok: true,
                        status_code: tonic::Code::Ok,
                        status_message: String::new(),
                    },
                    StreamFrameWire {
                        stream_id: stream_id.clone(),
                        direction: StreamDirection::ServerToClient,
                        seq: 2,
                        kind: StreamFrameKind::Close,
                        method: None,
                        metadata: Vec::new(),
                        payload: Bytes::new(),
                        status_ok: true,
                        status_code: tonic::Code::Ok,
                        status_message: String::new(),
                    },
                ] {
                    let bytes = encode_stream_frame(&frame);
                    let msg = TransportMessage::new(
                        0,
                        self.endpoint_id,
                        message.source,
                        bytes.to_vec(),
                        scheduler.time(),
                        MessageType::RpcStreamFrame,
                    )
                    .with_correlation_id(stream_id.clone());

                    scheduler.schedule(
                        SimTime::zero(),
                        self.transport_key,
                        TransportEvent::SendMessage { message: msg },
                    );
                }
            }
        }
    }

    #[test]
    fn client_streaming_returns_single_response() {
        std::thread::spawn(|| {
            let mut sim = Simulation::new(SimulationConfig { seed: 1 });
            des_tokio::runtime::install(&mut sim);

            let transport = crate::Transport::install_default(&mut sim);

            // Register a server endpoint and handler so the transport can route to it.
            let service_name = "svc".to_string();
            let server_endpoint = EndpointId::new(format!("{service_name}:server"));

            transport
                .endpoint_registry
                .register_endpoint(EndpointInfo {
                    id: server_endpoint,
                    service_name: service_name.clone(),
                    instance_name: "server".to_string(),
                    metadata: std::collections::HashMap::new(),
                })
                .unwrap();

            let server_key = sim.add_component(TestClientStreamingServer {
                endpoint_id: server_endpoint,
                transport_key: transport.transport_key,
            });

            {
                let t = sim
                    .get_component_mut::<TransportEvent, SimTransport>(transport.transport_key)
                    .unwrap();
                t.register_handler(server_endpoint, server_key);
            }

            let installed = ClientBuilder::new(
                service_name,
                transport.transport_key,
                transport.endpoint_registry.clone(),
                transport.scheduler.clone(),
            )
            .client_name("c1")
            .install(&mut sim)
            .unwrap();

            let channel = installed.channel.clone();

            let result: Arc<Mutex<Option<Result<Bytes, Status>>>> = Arc::new(Mutex::new(None));
            let result_out = result.clone();

            des_tokio::task::spawn_local(async move {
                let (sender, resp) = channel
                    .client_streaming("/svc.Test/ClientStreaming", Some(Duration::from_millis(50)))
                    .await
                    .unwrap();

                sender.send(Bytes::from_static(b"a")).await.unwrap();
                sender.send(Bytes::from_static(b"b")).await.unwrap();
                sender.close();

                let resp = resp.await;
                let bytes = resp.map(|r| r.into_inner());
                *result_out.lock().unwrap() = Some(bytes);
            });

            Executor::timed(SimTime::from_millis(20)).execute(&mut sim);

            let out = result.lock().unwrap().take().expect("result set");
            assert_eq!(out.unwrap(), Bytes::from_static(b"ok"));
        })
        .join()
        .unwrap();
    }

    #[test]
    fn client_streaming_accepts_implicit_open_from_server() {
        std::thread::spawn(|| {
            let mut sim = Simulation::new(SimulationConfig { seed: 1 });
            des_tokio::runtime::install(&mut sim);

            let transport = crate::Transport::install_default(&mut sim);

            // Register a server endpoint and handler so the transport can route to it.
            let service_name = "svc".to_string();
            let server_endpoint = EndpointId::new(format!("{service_name}:server"));

            transport
                .endpoint_registry
                .register_endpoint(EndpointInfo {
                    id: server_endpoint,
                    service_name: service_name.clone(),
                    instance_name: "server".to_string(),
                    metadata: std::collections::HashMap::new(),
                })
                .unwrap();

            let server_key = sim.add_component(TestClientStreamingServerNoOpen {
                endpoint_id: server_endpoint,
                transport_key: transport.transport_key,
            });

            {
                let t = sim
                    .get_component_mut::<TransportEvent, SimTransport>(transport.transport_key)
                    .unwrap();
                t.register_handler(server_endpoint, server_key);
            }

            let installed = ClientBuilder::new(
                service_name,
                transport.transport_key,
                transport.endpoint_registry.clone(),
                transport.scheduler.clone(),
            )
            .client_name("c1")
            .install(&mut sim)
            .unwrap();

            let channel = installed.channel.clone();

            let result: Arc<Mutex<Option<Result<Bytes, Status>>>> = Arc::new(Mutex::new(None));
            let result_out = result.clone();

            des_tokio::task::spawn_local(async move {
                let (sender, resp) = channel
                    .client_streaming("/svc.Test/ClientStreaming", Some(Duration::from_millis(50)))
                    .await
                    .unwrap();

                sender.send(Bytes::from_static(b"a")).await.unwrap();
                sender.close();

                let resp = resp.await;
                let bytes = resp.map(|r| r.into_inner());
                *result_out.lock().unwrap() = Some(bytes);
            });

            Executor::timed(SimTime::from_millis(20)).execute(&mut sim);

            let out = result.lock().unwrap().take().expect("result set");
            assert_eq!(out.unwrap(), Bytes::from_static(b"ok"));
        })
        .join()
        .unwrap();
    }
}

#[cfg(test)]
mod stream_protocol_tests {
    use super::*;
    use des_components::transport::{EndpointInfo, SimTransport};
    use des_core::{
        Component, Execute, Executor, Scheduler, SimTime, Simulation, SimulationConfig,
    };
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use tonic::Code;

    struct TestReorderingServer {
        endpoint_id: EndpointId,
        transport_key: Key<TransportEvent>,

        // If set, send an invalid RpcStreamFrame payload on Open.
        send_invalid_on_open: bool,
        // If set, on Close respond with a long sequence missing seq=1.
        overflow_on_close: bool,
        // If set, on Close respond with out-of-order Data frames.
        out_of_order_on_close: bool,
    }

    impl Component for TestReorderingServer {
        type Event = TransportEvent;

        fn process_event(
            &mut self,
            _self_id: Key<Self::Event>,
            event: &Self::Event,
            scheduler: &mut Scheduler,
        ) {
            let TransportEvent::MessageDelivered { message } = event else {
                return;
            };

            if message.message_type != MessageType::RpcStreamFrame {
                return;
            }

            let stream_id = match &message.correlation_id {
                Some(c) => c.clone(),
                None => return,
            };

            let frame = match decode_stream_frame(Bytes::from(message.payload.clone())) {
                Ok(f) => f,
                Err(_) => return,
            };

            if frame.stream_id != stream_id {
                return;
            }

            if frame.direction != StreamDirection::ClientToServer {
                return;
            }

            if self.send_invalid_on_open && frame.kind == StreamFrameKind::Open {
                let msg = TransportMessage::new(
                    0,
                    self.endpoint_id,
                    message.source,
                    vec![0u8],
                    scheduler.time(),
                    MessageType::RpcStreamFrame,
                )
                .with_correlation_id(stream_id);

                scheduler.schedule(
                    SimTime::zero(),
                    self.transport_key,
                    TransportEvent::SendMessage { message: msg },
                );
                return;
            }

            if frame.kind != StreamFrameKind::Close {
                return;
            }

            if self.out_of_order_on_close {
                let open = StreamFrameWire {
                    stream_id: stream_id.clone(),
                    direction: StreamDirection::ServerToClient,
                    seq: 0,
                    kind: StreamFrameKind::Open,
                    method: Some("/svc.Test/Download".to_string()),
                    metadata: Vec::new(),
                    payload: Bytes::new(),
                    status_ok: true,
                    status_code: tonic::Code::Ok,
                    status_message: String::new(),
                };

                let data2 = StreamFrameWire {
                    stream_id: stream_id.clone(),
                    direction: StreamDirection::ServerToClient,
                    seq: 2,
                    kind: StreamFrameKind::Data,
                    method: None,
                    metadata: Vec::new(),
                    payload: Bytes::from_static(b"b"),
                    status_ok: true,
                    status_code: tonic::Code::Ok,
                    status_message: String::new(),
                };

                let data1 = StreamFrameWire {
                    stream_id: stream_id.clone(),
                    direction: StreamDirection::ServerToClient,
                    seq: 1,
                    kind: StreamFrameKind::Data,
                    method: None,
                    metadata: Vec::new(),
                    payload: Bytes::from_static(b"a"),
                    status_ok: true,
                    status_code: tonic::Code::Ok,
                    status_message: String::new(),
                };

                let close = StreamFrameWire {
                    stream_id: stream_id.clone(),
                    direction: StreamDirection::ServerToClient,
                    seq: 3,
                    kind: StreamFrameKind::Close,
                    method: None,
                    metadata: Vec::new(),
                    payload: Bytes::new(),
                    status_ok: true,
                    status_code: tonic::Code::Ok,
                    status_message: String::new(),
                };

                for frame in [open, data2, data1, close] {
                    let msg = TransportMessage::new(
                        0,
                        self.endpoint_id,
                        message.source,
                        encode_stream_frame(&frame).to_vec(),
                        scheduler.time(),
                        MessageType::RpcStreamFrame,
                    )
                    .with_correlation_id(stream_id.clone());

                    scheduler.schedule(
                        SimTime::zero(),
                        self.transport_key,
                        TransportEvent::SendMessage { message: msg },
                    );
                }

                return;
            }

            if self.overflow_on_close {
                let open = StreamFrameWire {
                    stream_id: stream_id.clone(),
                    direction: StreamDirection::ServerToClient,
                    seq: 0,
                    kind: StreamFrameKind::Open,
                    method: Some("/svc.Test/Download".to_string()),
                    metadata: Vec::new(),
                    payload: Bytes::new(),
                    status_ok: true,
                    status_code: tonic::Code::Ok,
                    status_message: String::new(),
                };

                let msg = TransportMessage::new(
                    0,
                    self.endpoint_id,
                    message.source,
                    encode_stream_frame(&open).to_vec(),
                    scheduler.time(),
                    MessageType::RpcStreamFrame,
                )
                .with_correlation_id(stream_id.clone());

                scheduler.schedule(
                    SimTime::zero(),
                    self.transport_key,
                    TransportEvent::SendMessage { message: msg },
                );

                // Send enough out-of-order frames (missing seq=1) to exceed the client's reorder cap.
                for seq in 2u64..=66u64 {
                    let data = StreamFrameWire {
                        stream_id: stream_id.clone(),
                        direction: StreamDirection::ServerToClient,
                        seq,
                        kind: StreamFrameKind::Data,
                        method: None,
                        metadata: Vec::new(),
                        payload: Bytes::from_static(b"x"),
                        status_ok: true,
                        status_code: tonic::Code::Ok,
                        status_message: String::new(),
                    };

                    let msg = TransportMessage::new(
                        0,
                        self.endpoint_id,
                        message.source,
                        encode_stream_frame(&data).to_vec(),
                        scheduler.time(),
                        MessageType::RpcStreamFrame,
                    )
                    .with_correlation_id(stream_id.clone());

                    scheduler.schedule(
                        SimTime::zero(),
                        self.transport_key,
                        TransportEvent::SendMessage { message: msg },
                    );
                }
            }
        }
    }

    fn setup_sim_with_server(
        send_invalid_on_open: bool,
        overflow_on_close: bool,
        out_of_order_on_close: bool,
    ) -> (Simulation, Channel) {
        let mut sim = Simulation::new(SimulationConfig { seed: 1 });
        des_tokio::runtime::install(&mut sim);

        let transport = crate::Transport::install_default(&mut sim);

        let service_name = "svc".to_string();
        let server_endpoint = EndpointId::new(format!("{service_name}:server"));

        transport
            .endpoint_registry
            .register_endpoint(EndpointInfo {
                id: server_endpoint,
                service_name: service_name.clone(),
                instance_name: "server".to_string(),
                metadata: std::collections::HashMap::new(),
            })
            .unwrap();

        let server_key = sim.add_component(TestReorderingServer {
            endpoint_id: server_endpoint,
            transport_key: transport.transport_key,
            send_invalid_on_open,
            overflow_on_close,
            out_of_order_on_close,
        });

        {
            let t = sim
                .get_component_mut::<TransportEvent, SimTransport>(transport.transport_key)
                .unwrap();
            t.register_handler(server_endpoint, server_key);
        }

        let installed = ClientBuilder::new(
            service_name,
            transport.transport_key,
            transport.endpoint_registry.clone(),
            transport.scheduler.clone(),
        )
        .client_name("c1")
        .install(&mut sim)
        .unwrap();

        (sim, installed.channel)
    }

    #[test]
    fn server_streaming_reorders_out_of_order_frames() {
        std::thread::spawn(|| {
            let (mut sim, channel) = setup_sim_with_server(false, false, true);

            let out: Arc<Mutex<Option<Vec<Bytes>>>> = Arc::new(Mutex::new(None));
            let out2 = out.clone();

            des_tokio::task::spawn_local(async move {
                let resp = channel
                    .server_streaming(
                        "/svc.Test/Download",
                        Request::new(Bytes::from_static(b"req")),
                        Some(Duration::from_millis(50)),
                    )
                    .await
                    .unwrap();

                let mut stream = resp.into_inner();
                let mut items = Vec::new();
                while let Some(item) = stream.next().await {
                    items.push(item.unwrap());
                    if items.len() == 2 {
                        break;
                    }
                }

                *out2.lock().unwrap() = Some(items);
            });

            Executor::timed(SimTime::from_millis(50)).execute(&mut sim);

            assert_eq!(
                out.lock().unwrap().take(),
                Some(vec![Bytes::from_static(b"a"), Bytes::from_static(b"b")])
            );
        })
        .join()
        .unwrap();
    }

    #[test]
    fn server_streaming_reorder_buffer_overflow_reports_error() {
        std::thread::spawn(|| {
            let (mut sim, channel) = setup_sim_with_server(false, true, false);

            let out: Arc<Mutex<Option<Code>>> = Arc::new(Mutex::new(None));
            let out2 = out.clone();

            des_tokio::task::spawn_local(async move {
                let resp = channel
                    .server_streaming(
                        "/svc.Test/Download",
                        Request::new(Bytes::from_static(b"req")),
                        Some(Duration::from_millis(50)),
                    )
                    .await
                    .unwrap();

                let mut stream = resp.into_inner();
                let item = stream.next().await.expect("stream item");
                let code = item.unwrap_err().code();
                *out2.lock().unwrap() = Some(code);
            });

            Executor::timed(SimTime::from_millis(50)).execute(&mut sim);
            assert_eq!(out.lock().unwrap().take(), Some(Code::ResourceExhausted));
        })
        .join()
        .unwrap();
    }

    #[test]
    fn server_streaming_decode_error_fail_fast_reports_internal() {
        std::thread::spawn(|| {
            let (mut sim, channel) = setup_sim_with_server(true, false, false);

            let out: Arc<Mutex<Option<Code>>> = Arc::new(Mutex::new(None));
            let out2 = out.clone();

            des_tokio::task::spawn_local(async move {
                let resp = channel
                    .server_streaming(
                        "/svc.Test/Download",
                        Request::new(Bytes::from_static(b"req")),
                        Some(Duration::from_millis(50)),
                    )
                    .await
                    .unwrap();

                let mut stream = resp.into_inner();
                let item = stream.next().await.expect("stream item");
                let code = item.unwrap_err().code();
                *out2.lock().unwrap() = Some(code);
            });

            Executor::timed(SimTime::from_millis(50)).execute(&mut sim);
            assert_eq!(out.lock().unwrap().take(), Some(Code::Internal));
        })
        .join()
        .unwrap();
    }
}
