use crate::stream::DesStreamSender;
use crate::wire::{
    decode_stream_frame, decode_unary_response, encode_stream_frame, encode_unary_request,
    StreamDirection, StreamFrameKind, StreamFrameWire, UnaryRequestWire,
};
use bytes::Bytes;
use des_components::transport::{
    EndpointId, MessageType, SharedEndpointRegistry, SimTransport, TransportEvent, TransportMessage,
};
use des_core::{defer_wake, scheduler::in_scheduler_context, Key, SchedulerHandle, SimTime};

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

pub type ClientStreamingResponseFuture =
    Pin<Box<dyn Future<Output = Result<Response<Bytes>, Status>> + 'static>>;

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
    pending: Arc<
        Mutex<HashMap<String, des_tokio::sync::oneshot::Sender<Result<Response<Bytes>, Status>>>>,
    >,

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
        pending: Arc<
            Mutex<
                HashMap<String, des_tokio::sync::oneshot::Sender<Result<Response<Bytes>, Status>>>,
            >,
        >,
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
    /// Reserved for future streaming RPC support.
    #[allow(dead_code)]
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
            // Safe: defer_wake should not lock the scheduler.
            if delay != SimTime::zero() {
                // defer_wake only supports "now" semantics; fall back to scheduler handle.
                self.scheduler.schedule(delay, self.transport_key, event);
            } else {
                defer_wake(self.transport_key, event);
            }
        } else {
            self.scheduler.schedule(delay, self.transport_key, event);
        }
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

        let stream_id = self.next_stream_id();

        let (request_tx, mut request_rx) = des_tokio::sync::mpsc::channel::<Bytes>(16);
        let sender = DesStreamSender::new(request_tx);

        let (response_tx, response_rx) = des_tokio::sync::oneshot::channel();

        {
            let mut pending = self.pending_streams.lock().unwrap();
            pending.insert(
                stream_id.clone(),
                ClientStreamState {
                    mode: ClientStreamMode::ClientStreaming,
                    next_expected_s2c: 0,
                    reorder: BTreeMap::new(),
                    response_tx: Some(response_tx),
                    response_payload: None,
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

        let open_payload = encode_stream_frame(&open);
        let open_msg = TransportMessage::new(
            0,
            self.client_endpoint,
            target.id,
            open_payload.to_vec(),
            util::now(&self.scheduler),
            MessageType::RpcStreamFrame,
        )
        .with_correlation_id(stream_id.clone());

        self.schedule_transport(
            SimTime::zero(),
            TransportEvent::SendMessage { message: open_msg },
        );

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

                let bytes = encode_stream_frame(&frame);
                let msg = TransportMessage::new(
                    0,
                    channel.client_endpoint,
                    target_id_for_task,
                    bytes.to_vec(),
                    util::now(&channel.scheduler),
                    MessageType::RpcStreamFrame,
                )
                .with_correlation_id(stream_id_for_task.clone());

                channel.schedule_transport(
                    SimTime::zero(),
                    TransportEvent::SendMessage { message: msg },
                );
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

            let bytes = encode_stream_frame(&close);
            let msg = TransportMessage::new(
                0,
                channel.client_endpoint,
                target_id_for_task,
                bytes.to_vec(),
                util::now(&channel.scheduler),
                MessageType::RpcStreamFrame,
            )
            .with_correlation_id(stream_id_for_task);

            channel.schedule_transport(
                SimTime::zero(),
                TransportEvent::SendMessage { message: msg },
            );
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
                        let bytes = encode_stream_frame(&cancel);
                        let msg = TransportMessage::new(
                            0,
                            channel.client_endpoint,
                            target_id_for_task,
                            bytes.to_vec(),
                            util::now(&channel.scheduler),
                            MessageType::RpcStreamFrame,
                        )
                        .with_correlation_id(stream_id_for_response.clone());
                        channel.schedule_transport(
                            SimTime::zero(),
                            TransportEvent::SendMessage { message: msg },
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

    async fn unary_inner(
        &self,
        path: &str,
        request: Request<Bytes>,
    ) -> Result<Response<Bytes>, Status> {
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
            // Dynamic discovery: wait for an endpoint to become available.
            //
            // This mirrors tonic's load-balanced channels where requests can remain pending
            // while the balancer waits for endpoint updates.
            match des_tower::wait_for_endpoint(
                self.endpoint_registry.clone(),
                self.service_name.clone(),
                None,
            )
            .await
            {
                Some(ep) => ep,
                None => {
                    return Err(Status::deadline_exceeded(
                        "request timed out while waiting for endpoint",
                    ))
                }
            }
        };

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
    ClientStreaming,
}

pub(crate) struct ClientStreamState {
    mode: ClientStreamMode,
    next_expected_s2c: u64,
    reorder: BTreeMap<u64, StreamFrameWire>,
    response_tx: Option<des_tokio::sync::oneshot::Sender<Result<Response<Bytes>, Status>>>,
    response_payload: Option<Bytes>,
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
    pub pending: Arc<
        Mutex<HashMap<String, des_tokio::sync::oneshot::Sender<Result<Response<Bytes>, Status>>>>,
    >,
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
                    let stream_id = match &message.correlation_id {
                        Some(c) => c.clone(),
                        None => return,
                    };

                    let bytes = Bytes::from(message.payload.clone());
                    let frame = match decode_stream_frame(bytes) {
                        Ok(f) => f,
                        Err(_status) => return,
                    };

                    if frame.stream_id != stream_id {
                        return;
                    }

                    if frame.direction != StreamDirection::ServerToClient {
                        return;
                    }

                    let mut pending = self.pending_streams.lock().unwrap();
                    let Some(state) = pending.get_mut(&stream_id) else {
                        return;
                    };

                    // Reorder by seq and drain in-order.
                    state.reorder.insert(frame.seq, frame);

                    while let Some(next) = state.reorder.remove(&state.next_expected_s2c) {
                        state.next_expected_s2c += 1;

                        match next.kind {
                            StreamFrameKind::Open => {
                                // No-op for now.
                            }
                            StreamFrameKind::Data => match state.mode {
                                ClientStreamMode::ClientStreaming => {
                                    state.response_payload = Some(next.payload);
                                }
                            },
                            StreamFrameKind::Close => {
                                if let Some(tx) = state.response_tx.take() {
                                    if next.status_ok {
                                        let payload = state
                                            .response_payload
                                            .take()
                                            .unwrap_or_else(Bytes::new);
                                        let resp = Response::new(payload);
                                        let _ = tx.send(Ok(resp));
                                    } else {
                                        let status =
                                            Status::new(next.status_code, next.status_message);
                                        let _ = tx.send(Err(status));
                                    }
                                }

                                pending.remove(&stream_id);
                                break;
                            }
                            StreamFrameKind::Cancel => {
                                if let Some(tx) = state.response_tx.take() {
                                    let _ = tx.send(Err(Status::cancelled("stream cancelled")));
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
}
