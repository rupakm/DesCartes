use crate::router::{Router, ServerStreamingHandler};
use crate::stream::DesStreaming;
use crate::util;
use crate::wire::{
    decode_stream_frame, decode_unary_request, encode_stream_frame, encode_unary_response,
    StreamDirection, StreamFrameKind, StreamFrameWire, UnaryResponseWire,
};
use bytes::Bytes;
use descartes_components::transport::{
    EndpointId, EndpointInfo, MessageType, SharedEndpointRegistry, SimTransport, TransportEvent,
    TransportMessage,
};
use descartes_core::{Component, Key, Scheduler, SchedulerHandle, SimTime};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Duration;
use tonic::{
    metadata::{KeyAndValueRef, MetadataMap},
    Code, Request, Response, Status,
};

fn send_stream_frame_from_task(
    scheduler_handle: &SchedulerHandle,
    transport_key: Key<TransportEvent>,
    server_ep: EndpointId,
    client_ep: EndpointId,
    stream_id: &str,
    frame: StreamFrameWire,
) {
    let msg = TransportMessage::new(
        0,
        server_ep,
        client_ep,
        encode_stream_frame(&frame).to_vec(),
        util::now(scheduler_handle),
        MessageType::RpcStreamFrame,
    )
    .with_correlation_id(stream_id.to_string());

    if descartes_core::scheduler::in_scheduler_context() {
        descartes_core::defer_wake(transport_key, TransportEvent::SendMessage { message: msg });
    } else {
        scheduler_handle.schedule(
            SimTime::zero(),
            transport_key,
            TransportEvent::SendMessage { message: msg },
        );
    }
}

/// A server endpoint is a `des-core` component that receives `TransportEvent`s and
/// dispatches unary RPCs to a `Router`.
pub struct ServerEndpoint {
    pub name: String,
    pub endpoint_id: EndpointId,
    pub service_name: String,
    pub instance_name: String,
    pub transport_key: Key<TransportEvent>,
    pub endpoint_registry: SharedEndpointRegistry,
    pub router: Router,
    pub scheduler: SchedulerHandle,
    pub processing_delay: Duration,

    streams: HashMap<String, ServerStreamState>,
}

enum ServerStreamState {
    Client(ServerClientStreamState),
    Server(ServerServerStreamState),
    Bidi(ServerBidiStreamState),
}

struct ServerClientStreamState {
    next_expected_c2s: u64,
    reorder: BTreeMap<u64, StreamFrameWire>,
    inbound_tx: Option<descartes_tokio::sync::mpsc::Sender<Result<Bytes, Status>>>,
}

struct ServerServerStreamState {
    handler: ServerStreamingHandler,

    next_expected_c2s: u64,
    reorder: BTreeMap<u64, StreamFrameWire>,

    request_payload: Option<Bytes>,
    inbound_closed: bool,
    open_metadata: Vec<(String, String)>,
    method: String,
}

struct ServerBidiStreamState {
    next_expected_c2s: u64,
    reorder: BTreeMap<u64, StreamFrameWire>,
    inbound_tx: Option<descartes_tokio::sync::mpsc::Sender<Result<Bytes, Status>>>,
}

impl ServerClientStreamState {
    fn push_frame(&mut self, frame: StreamFrameWire) {
        self.reorder.insert(frame.seq, frame);
    }
}

impl ServerServerStreamState {
    fn push_frame(&mut self, frame: StreamFrameWire) {
        self.reorder.insert(frame.seq, frame);
    }
}

impl ServerBidiStreamState {
    fn push_frame(&mut self, frame: StreamFrameWire) {
        self.reorder.insert(frame.seq, frame);
    }
}

impl ServerEndpoint {
    pub fn start(&self) -> Result<(), Status> {
        let endpoint_info =
            EndpointInfo::new(self.service_name.clone(), self.instance_name.clone())
                .with_metadata("type".to_string(), "grpc".to_string())
                .with_metadata("endpoint_id".to_string(), self.endpoint_id.id().to_string());

        self.endpoint_registry
            .register_endpoint(endpoint_info)
            .map_err(|e| Status::internal(format!("failed to register endpoint: {e}")))?;

        Ok(())
    }
}

impl Component for ServerEndpoint {
    type Event = TransportEvent;

    #[allow(clippy::single_match)]
    fn process_event(
        &mut self,
        _self_id: Key<Self::Event>,
        event: &Self::Event,
        scheduler: &mut Scheduler,
    ) {
        let send_stream_frame = |scheduler: &mut Scheduler,
                                 transport_key: Key<TransportEvent>,
                                 server_ep: EndpointId,
                                 client_ep: EndpointId,
                                 stream_id: &str,
                                 frame: StreamFrameWire| {
            let msg = TransportMessage::new(
                0,
                server_ep,
                client_ep,
                encode_stream_frame(&frame).to_vec(),
                scheduler.time(),
                MessageType::RpcStreamFrame,
            )
            .with_correlation_id(stream_id.to_string());

            scheduler.schedule(
                SimTime::zero(),
                transport_key,
                TransportEvent::SendMessage { message: msg },
            );
        };

        match event {
            TransportEvent::MessageDelivered { message } => match message.message_type {
                MessageType::UnaryRequest => {
                    let correlation_id = match &message.correlation_id {
                        Some(c) => c.clone(),
                        None => return,
                    };

                    let router = self.router.clone();
                    let transport_key = self.transport_key;
                    let src = message.source;
                    let dst = message.destination;
                    let scheduler_handle = self.scheduler.clone();
                    let delay = self.processing_delay;

                    let encoded_req = Bytes::from(message.payload.clone());

                    // Spawn the handler on the DES async runtime.
                    descartes_tokio::task::spawn_local(async move {
                        if delay > Duration::ZERO {
                            descartes_tokio::time::sleep(delay).await;
                        }

                        let response_wire = match decode_unary_request(encoded_req) {
                            Ok(req_wire) => {
                                let handler = router.unary(&req_wire.method).map(Arc::clone);
                                match handler {
                                    Some(h) => {
                                        let mut req = Request::new(req_wire.payload);
                                        {
                                            let meta: &mut MetadataMap = req.metadata_mut();
                                            for (k, v) in req_wire.metadata {
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
                                        }

                                        match h(req).await {
                                            Ok(resp) => {
                                                let meta: Vec<(String, String)> = resp
                                                    .metadata()
                                                    .iter()
                                                    .filter_map(|kv| match kv {
                                                        KeyAndValueRef::Ascii(k, v) => Some((
                                                            k.as_str().to_string(),
                                                            v.to_str().ok()?.to_string(),
                                                        )),
                                                        _ => None,
                                                    })
                                                    .collect();

                                                UnaryResponseWire {
                                                    ok: true,
                                                    code: Code::Ok,
                                                    message: String::new(),
                                                    metadata: meta,
                                                    payload: resp.into_inner(),
                                                }
                                            }
                                            Err(status) => UnaryResponseWire {
                                                ok: false,
                                                code: status.code(),
                                                message: status.message().to_string(),
                                                metadata: Vec::new(),
                                                payload: Bytes::new(),
                                            },
                                        }
                                    }
                                    None => UnaryResponseWire {
                                        ok: false,
                                        code: Code::Unimplemented,
                                        message: format!("method not found: {}", req_wire.method),
                                        metadata: Vec::new(),
                                        payload: Bytes::new(),
                                    },
                                }
                            }
                            Err(status) => UnaryResponseWire {
                                ok: false,
                                code: status.code(),
                                message: status.message().to_string(),
                                metadata: Vec::new(),
                                payload: Bytes::new(),
                            },
                        };

                        let payload = encode_unary_response(&response_wire);

                        let response_msg = TransportMessage::new(
                            0,
                            dst,
                            src,
                            payload.to_vec(),
                            util::now(&scheduler_handle),
                            MessageType::UnaryResponse,
                        )
                        .with_correlation_id(correlation_id);

                        // Schedule send back through transport.
                        if descartes_core::scheduler::in_scheduler_context() {
                            descartes_core::defer_wake(
                                transport_key,
                                TransportEvent::SendMessage {
                                    message: response_msg,
                                },
                            );
                        } else {
                            scheduler_handle.schedule(
                                SimTime::zero(),
                                transport_key,
                                TransportEvent::SendMessage {
                                    message: response_msg,
                                },
                            );
                        }
                    });
                }
                MessageType::RpcStreamFrame => {
                    const MAX_REORDER_FRAMES: usize = 64;

                    let stream_id = match &message.correlation_id {
                        Some(c) => c.clone(),
                        None => return,
                    };

                    let mut frame = match decode_stream_frame(Bytes::from(message.payload.clone()))
                    {
                        Ok(f) => f,
                        Err(_) => return,
                    };

                    if frame.stream_id != stream_id {
                        frame.stream_id = stream_id.clone();
                    }

                    if frame.direction != StreamDirection::ClientToServer {
                        return;
                    }

                    let send_close = |scheduler: &mut Scheduler,
                                      transport_key: Key<TransportEvent>,
                                      server_ep: EndpointId,
                                      client_ep: EndpointId,
                                      stream_id: &str,
                                      status: Status| {
                        send_stream_frame(
                            scheduler,
                            transport_key,
                            server_ep,
                            client_ep,
                            stream_id,
                            StreamFrameWire {
                                stream_id: stream_id.to_string(),
                                direction: StreamDirection::ServerToClient,
                                seq: 1,
                                kind: StreamFrameKind::Close,
                                method: None,
                                metadata: Vec::new(),
                                payload: Bytes::new(),
                                status_ok: false,
                                status_code: status.code(),
                                status_message: status.message().to_string(),
                            },
                        )
                    };

                    match frame.kind {
                        StreamFrameKind::Open => {
                            let method = match frame.method.clone() {
                                Some(m) => m,
                                None => {
                                    send_close(
                                        scheduler,
                                        self.transport_key,
                                        self.endpoint_id,
                                        message.source,
                                        &stream_id,
                                        Status::invalid_argument("missing method"),
                                    );
                                    return;
                                }
                            };

                            if self.streams.contains_key(&stream_id) {
                                return;
                            }

                            if let Some(h) = self.router.bidi_streaming(&method).map(Arc::clone) {
                                let (in_tx, in_rx) =
                                    descartes_tokio::sync::mpsc::channel::<Result<Bytes, Status>>(16);

                                self.streams.insert(
                                    stream_id.clone(),
                                    ServerStreamState::Bidi(ServerBidiStreamState {
                                        // Open consumes seq=0.
                                        next_expected_c2s: 1,
                                        reorder: BTreeMap::new(),
                                        inbound_tx: Some(in_tx),
                                    }),
                                );

                                let stream_id_for_task = stream_id.clone();
                                let transport_key = self.transport_key;
                                let server_ep = self.endpoint_id;
                                let client_ep = message.source;
                                let scheduler_handle = self.scheduler.clone();
                                let delay = self.processing_delay;

                                let open_metadata = frame.metadata.clone();
                                let method_for_task = method.clone();

                                descartes_tokio::task::spawn_local(async move {
                                    if delay > Duration::ZERO {
                                        descartes_tokio::time::sleep(delay).await;
                                    }

                                    let mut req = Request::new(DesStreaming::new(in_rx));
                                    {
                                        let meta: &mut MetadataMap = req.metadata_mut();
                                        for (k, v) in open_metadata {
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
                                    }

                                    let result: Result<Response<DesStreaming<Bytes>>, Status> =
                                        h(req).await;

                                    let send_frame = |frame: StreamFrameWire| {
                                        send_stream_frame_from_task(
                                            &scheduler_handle,
                                            transport_key,
                                            server_ep,
                                            client_ep,
                                            &stream_id_for_task,
                                            frame,
                                        )
                                    };

                                    // Open S->C.
                                    send_frame(StreamFrameWire {
                                        stream_id: stream_id_for_task.clone(),
                                        direction: StreamDirection::ServerToClient,
                                        seq: 0,
                                        kind: StreamFrameKind::Open,
                                        method: Some(method_for_task.clone()),
                                        metadata: Vec::new(),
                                        payload: Bytes::new(),
                                        status_ok: true,
                                        status_code: tonic::Code::Ok,
                                        status_message: String::new(),
                                    });

                                    match result {
                                        Ok(resp) => {
                                            let mut stream = resp.into_inner();
                                            let mut seq = 1u64;

                                            while let Some(item) = stream.next().await {
                                                match item {
                                                    Ok(bytes) => {
                                                        send_frame(StreamFrameWire {
                                                            stream_id: stream_id_for_task.clone(),
                                                            direction:
                                                                StreamDirection::ServerToClient,
                                                            seq,
                                                            kind: StreamFrameKind::Data,
                                                            method: None,
                                                            metadata: Vec::new(),
                                                            payload: bytes,
                                                            status_ok: true,
                                                            status_code: tonic::Code::Ok,
                                                            status_message: String::new(),
                                                        });
                                                        seq += 1;
                                                    }
                                                    Err(status) => {
                                                        send_frame(StreamFrameWire {
                                                            stream_id: stream_id_for_task.clone(),
                                                            direction:
                                                                StreamDirection::ServerToClient,
                                                            seq,
                                                            kind: StreamFrameKind::Close,
                                                            method: None,
                                                            metadata: Vec::new(),
                                                            payload: Bytes::new(),
                                                            status_ok: false,
                                                            status_code: status.code(),
                                                            status_message: status
                                                                .message()
                                                                .to_string(),
                                                        });
                                                        return;
                                                    }
                                                }
                                            }

                                            send_frame(StreamFrameWire {
                                                stream_id: stream_id_for_task.clone(),
                                                direction: StreamDirection::ServerToClient,
                                                seq,
                                                kind: StreamFrameKind::Close,
                                                method: None,
                                                metadata: Vec::new(),
                                                payload: Bytes::new(),
                                                status_ok: true,
                                                status_code: tonic::Code::Ok,
                                                status_message: String::new(),
                                            });
                                        }
                                        Err(status) => {
                                            send_frame(StreamFrameWire {
                                                stream_id: stream_id_for_task.clone(),
                                                direction: StreamDirection::ServerToClient,
                                                seq: 1,
                                                kind: StreamFrameKind::Close,
                                                method: None,
                                                metadata: Vec::new(),
                                                payload: Bytes::new(),
                                                status_ok: false,
                                                status_code: status.code(),
                                                status_message: status.message().to_string(),
                                            });
                                        }
                                    }
                                });

                                return;
                            }

                            if let Some(h) = self.router.server_streaming(&method).map(Arc::clone) {
                                self.streams.insert(
                                    stream_id.clone(),
                                    ServerStreamState::Server(ServerServerStreamState {
                                        handler: h,
                                        // Open consumes seq=0.
                                        next_expected_c2s: 1,
                                        reorder: BTreeMap::new(),
                                        request_payload: None,
                                        inbound_closed: false,
                                        open_metadata: frame.metadata.clone(),
                                        method,
                                    }),
                                );
                                return;
                            }

                            if let Some(h) = self.router.client_streaming(&method).map(Arc::clone) {
                                let (in_tx, in_rx) =
                                    descartes_tokio::sync::mpsc::channel::<Result<Bytes, Status>>(16);

                                let handler = Arc::clone(&h);
                                let open_metadata = frame.metadata.clone();
                                let method_for_task = method.clone();

                                self.streams.insert(
                                    stream_id.clone(),
                                    ServerStreamState::Client(ServerClientStreamState {
                                        // Open consumes seq=0.
                                        next_expected_c2s: 1,
                                        reorder: BTreeMap::new(),
                                        inbound_tx: Some(in_tx),
                                    }),
                                );

                                // Spawn the handler task.
                                let stream_id_for_task = stream_id.clone();
                                let transport_key = self.transport_key;
                                let server_ep = self.endpoint_id;
                                let client_ep = message.source;
                                let scheduler_handle = self.scheduler.clone();
                                let delay = self.processing_delay;

                                descartes_tokio::task::spawn_local(async move {
                                    if delay > Duration::ZERO {
                                        descartes_tokio::time::sleep(delay).await;
                                    }

                                    let mut req = Request::new(DesStreaming::new(in_rx));
                                    {
                                        let meta: &mut MetadataMap = req.metadata_mut();
                                        for (k, v) in open_metadata {
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
                                    }

                                    let result: Result<Response<Bytes>, Status> =
                                        handler(req).await;

                                    let (data_payload, status_ok, status_code, status_message) =
                                        match result {
                                            Ok(resp) => (
                                                resp.into_inner(),
                                                true,
                                                tonic::Code::Ok,
                                                String::new(),
                                            ),
                                            Err(status) => (
                                                Bytes::new(),
                                                false,
                                                status.code(),
                                                status.message().to_string(),
                                            ),
                                        };

                                    let frames: Vec<StreamFrameWire> = if status_ok {
                                        vec![
                                            StreamFrameWire {
                                                stream_id: stream_id_for_task.clone(),
                                                direction: StreamDirection::ServerToClient,
                                                seq: 0,
                                                kind: StreamFrameKind::Open,
                                                method: Some(method_for_task.clone()),
                                                metadata: Vec::new(),
                                                payload: Bytes::new(),
                                                status_ok: true,
                                                status_code: tonic::Code::Ok,
                                                status_message: String::new(),
                                            },
                                            StreamFrameWire {
                                                stream_id: stream_id_for_task.clone(),
                                                direction: StreamDirection::ServerToClient,
                                                seq: 1,
                                                kind: StreamFrameKind::Data,
                                                method: None,
                                                metadata: Vec::new(),
                                                payload: data_payload,
                                                status_ok: true,
                                                status_code: tonic::Code::Ok,
                                                status_message: String::new(),
                                            },
                                            StreamFrameWire {
                                                stream_id: stream_id_for_task.clone(),
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
                                        ]
                                    } else {
                                        vec![
                                            StreamFrameWire {
                                                stream_id: stream_id_for_task.clone(),
                                                direction: StreamDirection::ServerToClient,
                                                seq: 0,
                                                kind: StreamFrameKind::Open,
                                                method: Some(method_for_task.clone()),
                                                metadata: Vec::new(),
                                                payload: Bytes::new(),
                                                status_ok: true,
                                                status_code: tonic::Code::Ok,
                                                status_message: String::new(),
                                            },
                                            StreamFrameWire {
                                                stream_id: stream_id_for_task.clone(),
                                                direction: StreamDirection::ServerToClient,
                                                seq: 1,
                                                kind: StreamFrameKind::Close,
                                                method: None,
                                                metadata: Vec::new(),
                                                payload: Bytes::new(),
                                                status_ok,
                                                status_code,
                                                status_message,
                                            },
                                        ]
                                    };

                                    for frame in frames {
                                        let msg = TransportMessage::new(
                                            0,
                                            server_ep,
                                            client_ep,
                                            encode_stream_frame(&frame).to_vec(),
                                            util::now(&scheduler_handle),
                                            MessageType::RpcStreamFrame,
                                        )
                                        .with_correlation_id(stream_id_for_task.clone());

                                        if descartes_core::scheduler::in_scheduler_context() {
                                            descartes_core::defer_wake(
                                                transport_key,
                                                TransportEvent::SendMessage { message: msg },
                                            );
                                        } else {
                                            scheduler_handle.schedule(
                                                SimTime::zero(),
                                                transport_key,
                                                TransportEvent::SendMessage { message: msg },
                                            );
                                        }
                                    }
                                });

                                return;
                            }

                            send_close(
                                scheduler,
                                self.transport_key,
                                self.endpoint_id,
                                message.source,
                                &stream_id,
                                Status::unimplemented(format!("method not found: {method}")),
                            );
                        }
                        _ => {
                            // Non-open frame: route to existing stream state.
                            let Some(state) = self.streams.get_mut(&stream_id) else {
                                return;
                            };

                            match state {
                                ServerStreamState::Client(state) => {
                                    let mut remove_stream = false;

                                    if state.reorder.len() >= MAX_REORDER_FRAMES {
                                        state.inbound_tx.take();
                                        remove_stream = true;
                                    } else {
                                        state.push_frame(frame);

                                        while let Some(next) =
                                            state.reorder.remove(&state.next_expected_c2s)
                                        {
                                            state.next_expected_c2s += 1;

                                            match next.kind {
                                                StreamFrameKind::Open => {}
                                                StreamFrameKind::Data => {
                                                    if let Some(tx) = &state.inbound_tx {
                                                        if tx.try_send(Ok(next.payload)).is_err() {
                                                            state.inbound_tx.take();
                                                            remove_stream = true;
                                                            break;
                                                        }
                                                    }
                                                }
                                                StreamFrameKind::Close => {
                                                    state.inbound_tx.take();
                                                }
                                                StreamFrameKind::Cancel => {
                                                    state.inbound_tx.take();
                                                    remove_stream = true;
                                                    break;
                                                }
                                            }
                                        }

                                        if state.inbound_tx.is_none() && state.reorder.is_empty() {
                                            remove_stream = true;
                                        }
                                    }

                                    if remove_stream {
                                        self.streams.remove(&stream_id);
                                    }
                                }
                                ServerStreamState::Bidi(state) => {
                                    let mut remove_stream = false;

                                    if state.reorder.len() >= MAX_REORDER_FRAMES {
                                        state.inbound_tx.take();
                                        remove_stream = true;
                                    } else {
                                        state.push_frame(frame);

                                        while let Some(next) =
                                            state.reorder.remove(&state.next_expected_c2s)
                                        {
                                            state.next_expected_c2s += 1;

                                            match next.kind {
                                                StreamFrameKind::Open => {}
                                                StreamFrameKind::Data => {
                                                    if let Some(tx) = &state.inbound_tx {
                                                        if tx.try_send(Ok(next.payload)).is_err() {
                                                            state.inbound_tx.take();
                                                            remove_stream = true;
                                                            break;
                                                        }
                                                    }
                                                }
                                                StreamFrameKind::Close => {
                                                    state.inbound_tx.take();
                                                }
                                                StreamFrameKind::Cancel => {
                                                    state.inbound_tx.take();
                                                    remove_stream = true;
                                                    break;
                                                }
                                            }
                                        }

                                        if state.inbound_tx.is_none() && state.reorder.is_empty() {
                                            remove_stream = true;
                                        }
                                    }

                                    if remove_stream {
                                        self.streams.remove(&stream_id);
                                    }
                                }
                                ServerStreamState::Server(state) => {
                                    if state.reorder.len() >= MAX_REORDER_FRAMES {
                                        self.streams.remove(&stream_id);
                                        return;
                                    }

                                    state.push_frame(frame);

                                    // Process in-order frames.
                                    while let Some(next) =
                                        state.reorder.remove(&state.next_expected_c2s)
                                    {
                                        state.next_expected_c2s += 1;

                                        match next.kind {
                                            StreamFrameKind::Open => {}
                                            StreamFrameKind::Data => {
                                                if state.request_payload.is_some() {
                                                    send_close(
                                                        scheduler,
                                                        self.transport_key,
                                                        self.endpoint_id,
                                                        message.source,
                                                        &stream_id,
                                                        Status::internal(
                                                            "multiple request data frames",
                                                        ),
                                                    );
                                                    self.streams.remove(&stream_id);
                                                    return;
                                                }
                                                state.request_payload = Some(next.payload);
                                            }
                                            StreamFrameKind::Close => {
                                                state.inbound_closed = true;
                                            }
                                            StreamFrameKind::Cancel => {
                                                self.streams.remove(&stream_id);
                                                return;
                                            }
                                        }
                                    }

                                    if state.inbound_closed {
                                        let Some(req_payload) = state.request_payload.clone()
                                        else {
                                            send_close(
                                                scheduler,
                                                self.transport_key,
                                                self.endpoint_id,
                                                message.source,
                                                &stream_id,
                                                Status::internal("missing request payload"),
                                            );
                                            self.streams.remove(&stream_id);
                                            return;
                                        };

                                        // Extract fields for the async task and remove state.
                                        let handler = Arc::clone(&state.handler);
                                        let method_for_task = state.method.clone();
                                        let open_metadata =
                                            std::mem::take(&mut state.open_metadata);
                                        self.streams.remove(&stream_id);

                                        let transport_key = self.transport_key;
                                        let server_ep = self.endpoint_id;
                                        let client_ep = message.source;
                                        let scheduler_handle = self.scheduler.clone();
                                        let delay = self.processing_delay;
                                        let stream_id_for_task = stream_id.clone();

                                        descartes_tokio::task::spawn_local(async move {
                                            if delay > Duration::ZERO {
                                                descartes_tokio::time::sleep(delay).await;
                                            }

                                            let mut req = Request::new(req_payload);
                                            {
                                                let meta: &mut MetadataMap = req.metadata_mut();
                                                for (k, v) in open_metadata {
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
                                            }

                                            let result: Result<
                                                Response<DesStreaming<Bytes>>,
                                                Status,
                                            > = handler(req).await;

                                            // Always send Open first.
                                            let open = StreamFrameWire {
                                                stream_id: stream_id_for_task.clone(),
                                                direction: StreamDirection::ServerToClient,
                                                seq: 0,
                                                kind: StreamFrameKind::Open,
                                                method: Some(method_for_task.clone()),
                                                metadata: Vec::new(),
                                                payload: Bytes::new(),
                                                status_ok: true,
                                                status_code: tonic::Code::Ok,
                                                status_message: String::new(),
                                            };

                                            let send_frame = |frame: StreamFrameWire| {
                                                send_stream_frame_from_task(
                                                    &scheduler_handle,
                                                    transport_key,
                                                    server_ep,
                                                    client_ep,
                                                    &stream_id_for_task,
                                                    frame,
                                                )
                                            };

                                            send_frame(open);

                                            match result {
                                                Ok(resp) => {
                                                    let mut stream = resp.into_inner();
                                                    let mut seq = 1u64;

                                                    while let Some(item) = stream.next().await {
                                                        match item {
                                                            Ok(bytes) => {
                                                                send_frame(StreamFrameWire {
                                                                    stream_id: stream_id_for_task.clone(),
                                                                    direction: StreamDirection::ServerToClient,
                                                                    seq,
                                                                    kind: StreamFrameKind::Data,
                                                                    method: None,
                                                                    metadata: Vec::new(),
                                                                    payload: bytes,
                                                                    status_ok: true,
                                                                    status_code: tonic::Code::Ok,
                                                                    status_message: String::new(),
                                                                });
                                                                seq += 1;
                                                            }
                                                            Err(status) => {
                                                                send_frame(StreamFrameWire {
                                                                    stream_id: stream_id_for_task.clone(),
                                                                    direction: StreamDirection::ServerToClient,
                                                                    seq,
                                                                    kind: StreamFrameKind::Close,
                                                                    method: None,
                                                                    metadata: Vec::new(),
                                                                    payload: Bytes::new(),
                                                                    status_ok: false,
                                                                    status_code: status.code(),
                                                                    status_message: status.message().to_string(),
                                                                });
                                                                return;
                                                            }
                                                        }
                                                    }

                                                    send_frame(StreamFrameWire {
                                                        stream_id: stream_id_for_task.clone(),
                                                        direction: StreamDirection::ServerToClient,
                                                        seq,
                                                        kind: StreamFrameKind::Close,
                                                        method: None,
                                                        metadata: Vec::new(),
                                                        payload: Bytes::new(),
                                                        status_ok: true,
                                                        status_code: tonic::Code::Ok,
                                                        status_message: String::new(),
                                                    });
                                                }
                                                Err(status) => {
                                                    send_frame(StreamFrameWire {
                                                        stream_id: stream_id_for_task.clone(),
                                                        direction: StreamDirection::ServerToClient,
                                                        seq: 1,
                                                        kind: StreamFrameKind::Close,
                                                        method: None,
                                                        metadata: Vec::new(),
                                                        payload: Bytes::new(),
                                                        status_ok: false,
                                                        status_code: status.code(),
                                                        status_message: status
                                                            .message()
                                                            .to_string(),
                                                    });
                                                }
                                            }
                                        });
                                    }
                                }
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

pub struct InstalledServer {
    pub endpoint_id: EndpointId,
    pub key: Key<TransportEvent>,
}

pub struct ServerBuilder {
    name: String,
    service_name: String,
    instance_name: String,
    transport_key: Key<TransportEvent>,
    endpoint_registry: SharedEndpointRegistry,
    router: Router,
    scheduler: SchedulerHandle,
    processing_delay: Duration,
}

impl ServerBuilder {
    pub fn new(
        service_name: String,
        instance_name: String,
        transport_key: Key<TransportEvent>,
        endpoint_registry: SharedEndpointRegistry,
        scheduler: SchedulerHandle,
    ) -> Self {
        Self {
            name: "server".to_string(),
            service_name,
            instance_name,
            transport_key,
            endpoint_registry,
            router: Router::new(),
            scheduler,
            processing_delay: Duration::from_millis(0),
        }
    }

    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    pub fn processing_delay(mut self, delay: Duration) -> Self {
        self.processing_delay = delay;
        self
    }

    pub fn add_router(mut self, router: Router) -> Self {
        self.router = router;
        self
    }

    pub fn build(self) -> ServerEndpoint {
        let endpoint_id = EndpointId::new(format!("{}:{}", self.service_name, self.instance_name));
        ServerEndpoint {
            name: self.name,
            endpoint_id,
            service_name: self.service_name,
            instance_name: self.instance_name,
            transport_key: self.transport_key,
            endpoint_registry: self.endpoint_registry,
            router: self.router,
            scheduler: self.scheduler,
            processing_delay: self.processing_delay,
            streams: HashMap::new(),
        }
    }
}

/// Convenience: register and add the server endpoint component.
impl ServerBuilder {
    pub fn install(self, sim: &mut descartes_core::Simulation) -> Result<InstalledServer, Status> {
        let endpoint = self.build();
        endpoint.start()?;
        let endpoint_id = endpoint.endpoint_id;
        let transport_key = endpoint.transport_key;
        let key = sim.add_component(endpoint);

        {
            let transport = sim
                .get_component_mut::<TransportEvent, SimTransport>(transport_key)
                .ok_or_else(|| Status::internal("transport component not found"))?;
            transport.register_handler(endpoint_id, key);
        }

        Ok(InstalledServer { endpoint_id, key })
    }
}
