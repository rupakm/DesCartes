//! Simulated axum server endpoint.
//!
//! A `ServerEndpoint` is a `des-core` component that receives `TransportEvent`s,
//! decodes an HTTP-shaped request, calls an axum `Router`, and sends an
//! HTTP-shaped response back to the request's source.

use crate::wire::{decode_request, encode_response, HttpRequestWire, HttpResponseWire};
use crate::Error;
use crate::util::{now, schedule_transport};
use des_components::transport::{
    EndpointId, EndpointInfo, MessageType, SharedEndpointRegistry, SimTransport, TransportEvent,
    TransportMessage,
};
use des_core::{Component, Key, Scheduler, SchedulerHandle};
use http::{HeaderName, HeaderValue, Method, Request, Response, Uri};
use http_body_util::BodyExt;
use std::time::Duration;
use tower::Service;

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
    app: axum::Router,
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
            app: axum::Router::new(),
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

    pub fn app(mut self, app: axum::Router) -> Self {
        self.app = app;
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
            app: self.app,
            scheduler: self.scheduler,
            processing_delay: self.processing_delay,
        }
    }

    pub fn install(self, sim: &mut des_core::Simulation) -> Result<InstalledServer, Error> {
        let endpoint = self.build();
        endpoint.start()?;
        let endpoint_id = endpoint.endpoint_id;
        let transport_key = endpoint.transport_key;
        let key = sim.add_component(endpoint);

        {
            let transport = sim
                .get_component_mut::<TransportEvent, SimTransport>(transport_key)
                .ok_or_else(|| Error::Internal("transport component not found".to_string()))?;
            transport.register_handler(endpoint_id, key);
        }

        Ok(InstalledServer { endpoint_id, key })
    }
}

pub struct ServerEndpoint {
    pub name: String,
    pub endpoint_id: EndpointId,
    pub service_name: String,
    pub instance_name: String,
    pub transport_key: Key<TransportEvent>,
    pub endpoint_registry: SharedEndpointRegistry,
    pub app: axum::Router,
    pub scheduler: SchedulerHandle,
    pub processing_delay: Duration,
}

impl ServerEndpoint {
    pub fn start(&self) -> Result<(), Error> {
        let endpoint_info = EndpointInfo::new(self.service_name.clone(), self.instance_name.clone())
            .with_metadata("type".to_string(), "http".to_string())
            .with_metadata("endpoint_id".to_string(), self.endpoint_id.id().to_string());

        self.endpoint_registry
            .register_endpoint(endpoint_info)
            .map_err(|e| {
                if e.contains("already registered") {
                    Error::AlreadyExists(e)
                } else {
                    Error::Internal(format!("failed to register endpoint: {e}"))
                }
            })?;

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

                let router = self.app.clone();
                let transport_key = self.transport_key;
                let src = message.source;
                let dst = message.destination;
                let scheduler_handle = self.scheduler.clone();
                let delay = self.processing_delay;

                let encoded_req = message.payload.clone();

                des_tokio::task::spawn_local(async move {
                    if delay > Duration::ZERO {
                        des_tokio::time::sleep(delay).await;
                    }

                    let resp_wire = match decode_request(&encoded_req) {
                        Ok(req_wire) => match request_from_wire(req_wire) {
                            Ok(req) => {
                                let mut svc = router;
                                match svc.call(req).await {
                                    Ok(resp) => response_to_wire(resp).await,
                                    Err(e) => Err(Error::Internal(format!("service error: {e}"))),
                                }
                            }
                            Err(e) => Err(e),
                        },
                        Err(e) => Err(Error::Internal(format!("decode: {e}"))),
                    };

                    let resp_wire = match resp_wire {
                        Ok(w) => w,
                        Err(e) => HttpResponseWire {
                            status: 500,
                            headers: vec![("content-type".to_string(), "text/plain".to_string())],
                            body: e.to_string().into_bytes(),
                        },
                    };

                    let payload = match encode_response(&resp_wire) {
                        Ok(p) => p,
                        Err(e) => {
                            let fallback = HttpResponseWire {
                                status: 500,
                                headers: vec![(
                                    "content-type".to_string(),
                                    "text/plain".to_string(),
                                )],
                                body: format!("encode: {e}").into_bytes(),
                            };
                            encode_response(&fallback).unwrap_or_default()
                        }
                    };

                    let msg = TransportMessage::new(
                        0,
                        dst,
                        src,
                        payload,
                        now(&scheduler_handle),
                        MessageType::UnaryResponse,
                    )
                    .with_correlation_id(correlation_id);

                    schedule_transport(
                        &scheduler_handle,
                        transport_key,
                        TransportEvent::SendMessage { message: msg },
                    );
                });
            }
            _ => {}
        }
    }
}

// shared helpers live in `crate::util`

fn request_from_wire(wire: HttpRequestWire) -> Result<Request<axum::body::Body>, Error> {
    let method = Method::from_bytes(wire.method.as_bytes())
        .map_err(|e| Error::InvalidArgument(format!("invalid method: {e}")))?;
    let uri = wire
        .uri
        .parse::<Uri>()
        .map_err(|e| Error::InvalidArgument(format!("invalid uri: {e}")))?;

    let mut builder = Request::builder().method(method).uri(uri);
    {
        let headers = builder
            .headers_mut()
            .ok_or_else(|| Error::Internal("request builder headers".to_string()))?;
        for (k, v) in wire.headers {
            let name = HeaderName::from_bytes(k.as_bytes())
                .map_err(|e| Error::InvalidArgument(format!("invalid header name: {e}")))?;
            let value = HeaderValue::from_str(&v)
                .map_err(|e| Error::InvalidArgument(format!("invalid header value: {e}")))?;
            headers.append(name, value);
        }
    }

    builder
        .body(axum::body::Body::from(wire.body))
        .map_err(|e| Error::InvalidArgument(format!("request build: {e}")))
}

async fn response_to_wire(resp: Response<axum::body::Body>) -> Result<HttpResponseWire, Error> {
    let (parts, body) = resp.into_parts();
    let body_bytes = body
        .collect()
        .await
        .map_err(|e| Error::Internal(format!("response body error: {e}")))?
        .to_bytes();

    let headers = parts
        .headers
        .iter()
        .map(|(k, v)| {
            (
                k.as_str().to_string(),
                v.to_str().unwrap_or("").to_string(),
            )
        })
        .collect::<Vec<_>>();

    Ok(HttpResponseWire {
        status: parts.status.as_u16(),
        headers,
        body: body_bytes.to_vec(),
    })
}
