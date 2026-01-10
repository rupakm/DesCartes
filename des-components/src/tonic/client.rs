//! Tonic-compatible RPC client implementation for discrete event simulation

use crate::tonic::{
    utils, MethodDescriptor, RpcCodec, RpcEvent, RpcRequest, RpcResponse, RpcStatus, RpcStatusCode,
    TonicError, TonicResult,
};
use crate::transport::{EndpointId, MessageType, SharedEndpointRegistry, TransportEvent};
use des_core::{Component, Key, Scheduler, SchedulerHandle, SimTime};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::oneshot;

/// Tonic-compatible RPC client for discrete event simulation
pub struct SimTonicClient<T> {
    /// Client endpoint ID
    endpoint_id: EndpointId,
    /// Target service name
    service_name: String,
    /// Transport component key
    transport_key: Key<TransportEvent>,
    /// Endpoint registry for service discovery
    endpoint_registry: SharedEndpointRegistry,
    /// Codec for message serialization
    codec: Box<dyn RpcCodec<T>>,
    /// Pending requests (correlation_id -> response_sender)
    pending_requests: Arc<Mutex<HashMap<String, oneshot::Sender<RpcResponse>>>>,
    /// Request timeout
    default_timeout: Duration,
    /// Request counter for correlation IDs
    request_counter: Arc<Mutex<u64>>,
}

impl<T> SimTonicClient<T>
where
    T: Send + Sync + 'static,
{
    /// Create a new tonic client
    pub fn new(
        service_name: String,
        transport_key: Key<TransportEvent>,
        endpoint_registry: SharedEndpointRegistry,
        codec: Box<dyn RpcCodec<T>>,
    ) -> Self {
        let endpoint_id = EndpointId::new(format!("client-{service_name}"));

        Self {
            endpoint_id,
            service_name,
            transport_key,
            endpoint_registry,
            codec,
            pending_requests: Arc::new(Mutex::new(HashMap::new())),
            default_timeout: Duration::from_secs(30),
            request_counter: Arc::new(Mutex::new(0)),
        }
    }

    /// Set default timeout for requests
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.default_timeout = timeout;
        self
    }

    /// Make a unary RPC call
    pub async fn unary_call(
        &self,
        method_name: &str,
        request: T,
        scheduler_handle: &SchedulerHandle,
    ) -> TonicResult<T> {
        // Find target endpoint
        let target_endpoint = self
            .endpoint_registry
            .get_endpoint_for_service(&self.service_name)
            .ok_or_else(|| TonicError::ServiceNotFound(self.service_name.clone()))?;

        // Encode request
        let request_payload = self.codec.encode(&request)?;

        // Create RPC request
        let method = MethodDescriptor::new(self.service_name.clone(), method_name.to_string());
        let rpc_request =
            RpcRequest::new(method, request_payload).with_timeout(self.default_timeout);

        // Generate correlation ID
        let correlation_id = {
            let mut counter = self.request_counter.lock().unwrap();
            *counter += 1;
            format!("req-{}-{}", self.endpoint_id.id(), *counter)
        };

        // Serialize request for transport
        let transport_payload = utils::serialize_rpc_request(&rpc_request);

        // Create response channel
        let (response_tx, response_rx) = oneshot::channel();

        // Store pending request
        {
            let mut pending = self.pending_requests.lock().unwrap();
            pending.insert(correlation_id.clone(), response_tx);
        }

        // Send request through transport
        // Note: In a real implementation, we'd need access to the transport component
        // For now, we'll simulate this by scheduling a transport event
        scheduler_handle.schedule_now(
            self.transport_key,
            TransportEvent::MessageDelivered {
                message: crate::transport::TransportMessage::new(
                    0, // message ID will be assigned by transport
                    self.endpoint_id,
                    target_endpoint.id,
                    transport_payload,
                    scheduler_handle.time(),
                    MessageType::UnaryRequest,
                )
                .with_correlation_id(correlation_id.clone()),
                destination: target_endpoint.id,
            },
        );

        // Schedule timeout
        let client_key = Key::new_with_id(uuid::Uuid::new_v4()); // Generate a dummy key for timeout
        scheduler_handle.schedule(
            SimTime::from_duration(self.default_timeout),
            client_key,
            RpcEvent::RequestTimeout {
                correlation_id: correlation_id.clone(),
            },
        );

        // Wait for response
        let rpc_response = response_rx
            .await
            .map_err(|_| TonicError::Internal("Response channel closed".to_string()))?;

        // Check response status
        match rpc_response.status {
            RpcStatus::Ok => {
                // Decode response
                self.codec.decode(&rpc_response.payload)
            }
            RpcStatus::Error { code, message } => Err(TonicError::RpcFailed { code, message }),
        }
    }

    /// Handle incoming RPC response
    pub fn handle_response(&self, response: RpcResponse, correlation_id: String) {
        let mut pending = self.pending_requests.lock().unwrap();
        if let Some(response_tx) = pending.remove(&correlation_id) {
            let _ = response_tx.send(response);
        }
    }

    /// Handle request timeout
    pub fn handle_timeout(&self, correlation_id: String) {
        let mut pending = self.pending_requests.lock().unwrap();
        if let Some(response_tx) = pending.remove(&correlation_id) {
            let timeout_response = RpcResponse::error(
                RpcStatusCode::DeadlineExceeded,
                "Request timeout".to_string(),
            );
            let _ = response_tx.send(timeout_response);
        }
    }

    /// Get client endpoint ID
    pub fn endpoint_id(&self) -> EndpointId {
        self.endpoint_id
    }
}

/// Component wrapper for SimTonicClient to handle RPC events
pub struct TonicClientComponent<T> {
    client: SimTonicClient<T>,
}

impl<T> TonicClientComponent<T>
where
    T: Send + Sync + 'static,
{
    /// Create a new client component
    pub fn new(client: SimTonicClient<T>) -> Self {
        Self { client }
    }

    /// Get reference to the underlying client
    pub fn client(&self) -> &SimTonicClient<T> {
        &self.client
    }
}

impl<T> Component for TonicClientComponent<T>
where
    T: Send + Sync + 'static,
{
    type Event = RpcEvent;

    fn process_event(
        &mut self,
        _self_id: Key<Self::Event>,
        event: &Self::Event,
        _scheduler: &mut Scheduler,
    ) {
        match event {
            RpcEvent::ResponseReceived {
                response,
                correlation_id,
            } => {
                self.client
                    .handle_response(response.clone(), correlation_id.clone());
            }
            RpcEvent::RequestTimeout { correlation_id } => {
                self.client.handle_timeout(correlation_id.clone());
            }
            _ => {
                // Ignore other events
            }
        }
    }
}

/// Builder for creating tonic clients
pub struct TonicClientBuilder<T> {
    service_name: Option<String>,
    transport_key: Option<Key<TransportEvent>>,
    endpoint_registry: Option<SharedEndpointRegistry>,
    codec: Option<Box<dyn RpcCodec<T>>>,
    timeout: Duration,
}

impl<T> TonicClientBuilder<T>
where
    T: Send + Sync + 'static,
{
    /// Create a new client builder
    pub fn new() -> Self {
        Self {
            service_name: None,
            transport_key: None,
            endpoint_registry: None,
            codec: None,
            timeout: Duration::from_secs(30),
        }
    }

    /// Set the service name
    pub fn service_name(mut self, service_name: String) -> Self {
        self.service_name = Some(service_name);
        self
    }

    /// Set the transport component key
    pub fn transport_key(mut self, transport_key: Key<TransportEvent>) -> Self {
        self.transport_key = Some(transport_key);
        self
    }

    /// Set the endpoint registry
    pub fn endpoint_registry(mut self, endpoint_registry: SharedEndpointRegistry) -> Self {
        self.endpoint_registry = Some(endpoint_registry);
        self
    }

    /// Set the codec
    pub fn codec(mut self, codec: Box<dyn RpcCodec<T>>) -> Self {
        self.codec = Some(codec);
        self
    }

    /// Set the default timeout
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Build the client
    pub fn build(self) -> TonicResult<SimTonicClient<T>> {
        let service_name = self
            .service_name
            .ok_or_else(|| TonicError::Internal("Service name is required".to_string()))?;
        let transport_key = self
            .transport_key
            .ok_or_else(|| TonicError::Internal("Transport key is required".to_string()))?;
        let endpoint_registry = self
            .endpoint_registry
            .ok_or_else(|| TonicError::Internal("Endpoint registry is required".to_string()))?;
        let codec = self
            .codec
            .ok_or_else(|| TonicError::Internal("Codec is required".to_string()))?;

        Ok(
            SimTonicClient::new(service_name, transport_key, endpoint_registry, codec)
                .with_timeout(self.timeout),
        )
    }
}

impl<T> Default for TonicClientBuilder<T>
where
    T: Send + Sync + 'static,
{
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tonic::codec::JsonCodec;
    use crate::transport::{SimTransport, SimpleNetworkModel};
    use des_core::Simulation;
    use serde::{Deserialize, Serialize};

    #[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
    struct TestRequest {
        id: u32,
        name: String,
    }

    #[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
    struct TestResponse {
        result: String,
    }

    #[test]
    fn test_client_creation() {
        let mut sim = Simulation::default();

        // Create transport
        let network_model = Box::new(SimpleNetworkModel::new(Duration::from_millis(10), 0.0));
        let transport = SimTransport::new(network_model);
        let transport_key = sim.add_component(transport);

        // Create client
        let client = TonicClientBuilder::<TestRequest>::new()
            .service_name("test.Service".to_string())
            .transport_key(transport_key)
            .endpoint_registry(SharedEndpointRegistry::new())
            .codec(Box::new(JsonCodec::new()))
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap();

        assert_eq!(client.service_name, "test.Service");
        assert_eq!(client.default_timeout, Duration::from_secs(5));
    }

    #[test]
    fn test_client_builder_validation() {
        // Missing service name
        let result = TonicClientBuilder::<TestRequest>::new().build();
        assert!(result.is_err());

        // Missing transport key
        let result = TonicClientBuilder::<TestRequest>::new()
            .service_name("test.Service".to_string())
            .build();
        assert!(result.is_err());
    }
}
