use crate::wire::{decode_unary_response, encode_unary_request, UnaryRequestWire};
use bytes::Bytes;
use des_components::transport::{
    EndpointId, MessageType, SharedEndpointRegistry, SimTransport, TransportEvent, TransportMessage,
};
use des_core::{defer_wake, scheduler::in_scheduler_context, Key, SchedulerHandle, SimTime};

use crate::util;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tonic::{
    metadata::{KeyAndValueRef, MetadataMap},
    Request, Response, Status,
};

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

    next_counter: Arc<Mutex<u64>>,
}

impl Channel {
    /// Create a `Channel` bound to a particular service name.
    pub fn new(
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
    ) -> Self {
        Self {
            service_name,
            transport_key,
            endpoint_registry,
            scheduler,
            client_endpoint,
            pending,
            next_counter: Arc::new(Mutex::new(0)),
        }
    }

    pub fn client_endpoint(&self) -> EndpointId {
        self.client_endpoint
    }

    fn next_correlation_id(&self) -> String {
        let mut c = self.next_counter.lock().unwrap();
        *c += 1;
        format!("{}:{}:{}", self.service_name, self.client_endpoint.id(), *c)
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
        let target = self
            .endpoint_registry
            .get_endpoint_for_service(&self.service_name)
            .ok_or_else(|| {
                Status::unavailable(format!("service not found: {}", self.service_name))
            })?;

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

        let result = if let Some(timeout) = timeout {
            match des_tokio::time::timeout(timeout, rx).await {
                Ok(r) => r,
                Err(_) => Err(des_tokio::sync::oneshot::RecvError),
            }
        } else {
            rx.await
        };

        match result {
            Ok(Ok(resp)) => Ok(resp),
            Ok(Err(status)) => Err(status),
            Err(_) => {
                // Timeout or channel closed.
                let mut pending = self.pending.lock().unwrap();
                pending.remove(&correlation_id);
                Err(Status::deadline_exceeded("request timed out"))
            }
        }
    }
}

/// Installed client artifacts.
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
}

impl ClientEndpoint {
    pub fn new(endpoint_id: EndpointId) -> Self {
        Self {
            endpoint_id,
            pending: Arc::new(Mutex::new(HashMap::new())),
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
            TransportEvent::MessageDelivered { message } => {
                if message.message_type != MessageType::UnaryResponse {
                    return;
                }

                let correlation_id = match &message.correlation_id {
                    Some(c) => c.clone(),
                    None => return,
                };

                let bytes = Bytes::from(message.payload.clone());
                let decoded = decode_unary_response(bytes);

                let mut pending = self.pending.lock().unwrap();
                if let Some(tx) = pending.remove(&correlation_id) {
                    let _ = match decoded {
                        Ok(wire) => {
                            if wire.ok {
                                let mut resp = Response::new(wire.payload);
                                let meta: &mut MetadataMap = resp.metadata_mut();
                                for (k, v) in wire.metadata {
                                    if let (Ok(k), Ok(v)) = (
                                        k.parse::<tonic::metadata::MetadataKey<tonic::metadata::Ascii>>(),
                                        v.parse::<tonic::metadata::MetadataValue<tonic::metadata::Ascii>>(),
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
            _ => {}
        }
    }
}
