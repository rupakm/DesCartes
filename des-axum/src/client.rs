//! Simulated axum client.
//!
//! The client sends HTTP-shaped requests over `SimTransport` using a correlation ID
//! and resolves matching responses in the `ClientEndpoint` component.
//!
//! Bodies are currently handled as collected bytes.

use crate::wire::{decode_response, encode_request, HttpRequestWire, HttpResponseWire};
use crate::util::{now, schedule_transport};
use descartes_components::transport::{
    EndpointId, EndpointInfo, MessageType, SharedEndpointRegistry, SimTransport, TransportEvent,
    TransportMessage,
};
use descartes_core::{Component, Key, Scheduler, SchedulerHandle};
use http::{HeaderName, HeaderValue, Method, Request, Response, Uri};
use http_body_util::BodyExt;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Debug, thiserror::Error, Clone)]
pub enum Error {
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("already exists: {0}")]
    AlreadyExists(String),
    #[error("timeout: {0}")]
    Timeout(String),
    #[error("cancelled")]
    Cancelled,
    #[error("internal error: {0}")]
    Internal(String),
}

type Pending = Arc<
    Mutex<
        HashMap<
            String,
            descartes_tokio::sync::oneshot::Sender<Result<Response<axum::body::Body>, Error>>,
        >,
    >,
>;

pub struct InstalledClient {
    pub endpoint_id: EndpointId,
    pub key: Key<TransportEvent>,
    pub client: Client,
}

pub struct ClientBuilder {
    service_name: String,
    transport_key: Key<TransportEvent>,
    endpoint_registry: SharedEndpointRegistry,
    scheduler: SchedulerHandle,
    client_name: String,
    endpoint_id: Option<EndpointId>,
    target_endpoint: Option<EndpointId>,
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
            target_endpoint: None,
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

    /// Pin this client to a specific server endpoint.
    pub fn target_endpoint(mut self, endpoint: EndpointId) -> Self {
        self.target_endpoint = Some(endpoint);
        self
    }

    pub fn install(self, sim: &mut descartes_core::Simulation) -> Result<InstalledClient, Error> {
        let endpoint_id = self
            .endpoint_id
            .unwrap_or_else(|| EndpointId::new(format!("client:{}", self.client_name)));

        let client_endpoint = ClientEndpoint::new(endpoint_id);
        let pending = client_endpoint.pending.clone();
        let client_key = sim.add_component(client_endpoint);

        {
            let transport = sim
                .get_component_mut::<TransportEvent, SimTransport>(self.transport_key)
                .ok_or_else(|| Error::Internal("transport component not found".to_string()))?;
            transport.register_handler(endpoint_id, client_key);
        }

        let client = Client::new(
            self.service_name,
            self.transport_key,
            self.endpoint_registry,
            self.scheduler,
            endpoint_id,
            pending,
            self.target_endpoint,
        );

        Ok(InstalledClient {
            endpoint_id,
            key: client_key,
            client,
        })
    }
}

pub struct Client {
    service_name: String,
    transport_key: Key<TransportEvent>,
    endpoint_registry: SharedEndpointRegistry,
    scheduler: SchedulerHandle,
    client_endpoint: EndpointId,
    pending: Pending,
    next_counter: Arc<Mutex<u64>>,
    target_endpoint: Option<EndpointId>,
}

impl Client {
    fn new(
        service_name: String,
        transport_key: Key<TransportEvent>,
        endpoint_registry: SharedEndpointRegistry,
        scheduler: SchedulerHandle,
        client_endpoint: EndpointId,
        pending: Pending,
        target_endpoint: Option<EndpointId>,
    ) -> Self {
        Self {
            service_name,
            transport_key,
            endpoint_registry,
            scheduler,
            client_endpoint,
            pending,
            next_counter: Arc::new(Mutex::new(0)),
            target_endpoint,
        }
    }

    fn next_correlation_id(&self) -> String {
        let mut c = self.next_counter.lock().unwrap();
        *c += 1;
        format!("{}:{}:{}", self.service_name, self.client_endpoint.id(), *c)
    }

    async fn select_target(
        &self,
        timeout: Option<Duration>,
    ) -> Result<EndpointInfo, Error> {
        if let Some(target) = self.target_endpoint {
            let found = self
                .endpoint_registry
                .find_endpoints(&self.service_name)
                .into_iter()
                .find(|e| e.id == target);
            return found.ok_or_else(|| {
                Error::NotFound(format!("service not found at pinned endpoint ({})", target.id()))
            });
        }

        let deadline = timeout.map(|d| descartes_tokio::time::Instant::now() + d);
        loop {
            if let Some(ep) = self.endpoint_registry.get_endpoint_for_service(&self.service_name) {
                return Ok(ep);
            }

            match deadline {
                None => {
                    self.endpoint_registry.changed().await;
                }
                Some(d) => {
                    let now = descartes_tokio::time::Instant::now();
                    if now >= d {
                        return Err(Error::Timeout(format!(
                            "no endpoints for '{}'",
                            self.service_name
                        )));
                    }

                    let remaining = d.saturating_duration_since(now);
                    if descartes_tokio::time::timeout(remaining, self.endpoint_registry.changed())
                        .await
                        .is_err()
                    {
                        return Err(Error::Timeout(format!(
                            "no endpoints for '{}'",
                            self.service_name
                        )));
                    }
                }
            }
        }
    }

    pub async fn request(
        &self,
        req: Request<axum::body::Body>,
        timeout: Option<Duration>,
    ) -> Result<Response<axum::body::Body>, Error> {
        let correlation_id = self.next_correlation_id();
        let ep = self.select_target(timeout).await?;

        let (parts, body) = req.into_parts();
        let body_bytes = body
            .collect()
            .await
            .map_err(|e| Error::Internal(format!("request body error: {e}")))?
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

        let req_wire = HttpRequestWire {
            method: parts.method.as_str().to_string(),
            uri: parts.uri.to_string(),
            headers,
            body: body_bytes.to_vec(),
        };

        let payload =
            encode_request(&req_wire).map_err(|e| Error::Internal(format!("encode: {e}")))?;

        let msg = TransportMessage::new(
            0,
            self.client_endpoint,
            ep.id,
            payload,
            now(&self.scheduler),
            MessageType::UnaryRequest,
        )
        .with_correlation_id(correlation_id.clone());

        let (tx, rx) = descartes_tokio::sync::oneshot::channel();
        {
            let mut pending = self.pending.lock().unwrap();
            pending.insert(correlation_id.clone(), tx);
        }

        schedule_transport(
            &self.scheduler,
            self.transport_key,
            TransportEvent::SendMessage { message: msg },
        );

        let result = match timeout {
            None => rx
                .await
                .map_err(|_| Error::Cancelled)
                .and_then(|r| r),
            Some(d) => match descartes_tokio::time::timeout(d, rx).await {
                Ok(r) => r.map_err(|_| Error::Cancelled).and_then(|r| r),
                Err(_) => {
                    let mut pending = self.pending.lock().unwrap();
                    pending.remove(&correlation_id);
                    Err(Error::Timeout(format!("request timed out after {d:?}")))
                }
            },
        };

        result
    }

    pub async fn get(
        &self,
        uri: impl AsRef<str>,
        timeout: Option<Duration>,
    ) -> Result<Response<axum::body::Body>, Error> {
        let req = Request::builder()
            .method(Method::GET)
            .uri(
                uri.as_ref()
                    .parse::<Uri>()
                    .map_err(|e| Error::InvalidArgument(format!("invalid uri: {e}")))?,
            )
            .body(axum::body::Body::empty())
            .map_err(|e| Error::InvalidArgument(format!("request build: {e}")))?;
        self.request(req, timeout).await
    }
}

pub struct ClientEndpoint {
    endpoint_id: EndpointId,
    pub pending: Pending,
}

impl ClientEndpoint {
    pub fn new(endpoint_id: EndpointId) -> Self {
        Self {
            endpoint_id,
            pending: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl Component for ClientEndpoint {
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
                if message.message_type != MessageType::UnaryResponse {
                    return;
                }

                if message.destination != self.endpoint_id {
                    return;
                }

                let correlation_id = match &message.correlation_id {
                    Some(c) => c.clone(),
                    None => return,
                };

                let decoded = decode_response(&message.payload);
                let mut pending = self.pending.lock().unwrap();
                let Some(tx) = pending.remove(&correlation_id) else {
                    return;
                };

                let result = decoded
                    .map_err(|e| Error::Internal(format!("decode: {e}")))
                    .and_then(http_response_from_wire);

                let _ = tx.send(result);
            }
            _ => {}
        }
    }
}

fn http_response_from_wire(wire: HttpResponseWire) -> Result<Response<axum::body::Body>, Error> {
    let mut builder = Response::builder().status(wire.status);
    {
        let headers = builder
            .headers_mut()
            .ok_or_else(|| Error::Internal("response builder headers".to_string()))?;
        for (k, v) in wire.headers {
            let name = HeaderName::from_bytes(k.as_bytes())
                .map_err(|e| Error::Internal(format!("header name: {e}")))?;
            let value = HeaderValue::from_str(&v)
                .map_err(|e| Error::Internal(format!("header value: {e}")))?;
            headers.append(name, value);
        }
    }

    builder
        .body(axum::body::Body::from(wire.body))
        .map_err(|e| Error::Internal(format!("response build: {e}")))
}

// shared helpers live in `crate::util`
