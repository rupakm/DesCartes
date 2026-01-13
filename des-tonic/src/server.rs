use crate::router::Router;
use crate::util;
use crate::wire::{decode_unary_request, encode_unary_response, UnaryResponseWire};
use bytes::Bytes;
use des_components::transport::{
    EndpointId, EndpointInfo, MessageType, SharedEndpointRegistry, SimTransport, TransportEvent,
    TransportMessage,
};
use des_core::{Component, Key, Scheduler, SchedulerHandle, SimTime};
use std::sync::Arc;
use std::time::Duration;
use tonic::{
    metadata::{KeyAndValueRef, MetadataMap},
    Code, Request, Status,
};

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

    fn process_event(
        &mut self,
        _self_id: Key<Self::Event>,
        event: &Self::Event,
        _scheduler: &mut Scheduler,
    ) {
        match event {
            TransportEvent::MessageDelivered { message } => {
                if message.message_type != MessageType::UnaryRequest {
                    return;
                }

                let correlation_id = match &message.correlation_id {
                    Some(c) => c.clone(),
                    None => return,
                };

                let router = self.router.clone();
                let transport_key = self.transport_key;
                let src = message.source;
                let dst = message.destination;
                let scheduler = self.scheduler.clone();
                let delay = self.processing_delay;

                let encoded_req = Bytes::from(message.payload.clone());

                // Spawn the handler on the DES async runtime.
                des_tokio::task::spawn_local(async move {
                    if delay > Duration::ZERO {
                        des_tokio::time::sleep(delay).await;
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
                        util::now(&scheduler),
                        MessageType::UnaryResponse,
                    )
                    .with_correlation_id(correlation_id);

                    // Schedule send back through transport.
                    if des_core::scheduler::in_scheduler_context() {
                        des_core::defer_wake(
                            transport_key,
                            TransportEvent::SendMessage {
                                message: response_msg,
                            },
                        );
                    } else {
                        scheduler.schedule(
                            SimTime::zero(),
                            transport_key,
                            TransportEvent::SendMessage {
                                message: response_msg,
                            },
                        );
                    }
                });
            }
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
        }
    }
}

/// Convenience: register and add the server endpoint component.
impl ServerBuilder {
    pub fn install(self, sim: &mut des_core::Simulation) -> Result<InstalledServer, Status> {
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
