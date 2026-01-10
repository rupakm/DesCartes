//! DES-integrated tonic client component for periodic RPC requests

use crate::tonic::{
    utils, MethodDescriptor, RpcCodec, RpcRequest, RpcResponse, RpcStatus, TonicError,
};
use crate::transport::{
    EndpointId, MessageType, SharedEndpointRegistry, TransportEvent, TransportMessage,
};
use des_core::{Component, Key, Scheduler, SimTime};
use std::collections::HashMap;
use std::time::Duration;

/// Events for the DES tonic client
#[derive(Debug, Clone)]
pub enum TonicClientEvent {
    /// Time to send the next periodic request
    SendPeriodicRequest,
    /// Response received from server
    ResponseReceived {
        response: RpcResponse,
        correlation_id: String,
    },
    /// Request timed out
    RequestTimeout { correlation_id: String },
}

/// DES component that wraps a tonic client and sends periodic requests
pub struct DesTonicClient<T> {
    /// Client name for identification
    pub name: String,
    /// Client endpoint ID
    pub endpoint_id: EndpointId,
    /// Target service name
    pub service_name: String,
    /// Method to call
    pub method_name: String,
    /// Transport component key
    pub transport_key: Key<TransportEvent>,
    /// Endpoint registry for service discovery
    pub endpoint_registry: SharedEndpointRegistry,
    /// Codec for message serialization
    pub codec: Box<dyn RpcCodec<T>>,
    /// Request interval for periodic requests
    pub request_interval: Duration,
    /// Request timeout
    pub request_timeout: Duration,
    /// Request counter for correlation IDs
    pub request_counter: u64,
    /// Pending requests (correlation_id -> sent_time)
    pub pending_requests: HashMap<String, SimTime>,
    /// Statistics
    pub requests_sent: u64,
    pub responses_received: u64,
    pub requests_timed_out: u64,
    pub successful_responses: u64,
    pub failed_responses: u64,
    /// Request payload generator
    pub request_generator: Box<dyn Fn(u64) -> T + Send + Sync>,
}

impl<T> DesTonicClient<T>
where
    T: Send + Sync + 'static,
{
    /// Create a new DES tonic client
    pub fn new(
        name: String,
        service_name: String,
        method_name: String,
        transport_key: Key<TransportEvent>,
        endpoint_registry: SharedEndpointRegistry,
        codec: Box<dyn RpcCodec<T>>,
        request_generator: Box<dyn Fn(u64) -> T + Send + Sync>,
    ) -> Self {
        let endpoint_id = EndpointId::new(format!("client-{}", name));

        Self {
            name,
            endpoint_id,
            service_name,
            method_name,
            transport_key,
            endpoint_registry,
            codec,
            request_interval: Duration::from_secs(1),
            request_timeout: Duration::from_secs(5),
            request_counter: 0,
            pending_requests: HashMap::new(),
            requests_sent: 0,
            responses_received: 0,
            requests_timed_out: 0,
            successful_responses: 0,
            failed_responses: 0,
            request_generator,
        }
    }

    /// Set request interval
    pub fn with_interval(mut self, interval: Duration) -> Self {
        self.request_interval = interval;
        self
    }

    /// Set request timeout
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = timeout;
        self
    }

    /// Send a request to the server
    fn send_request(
        &mut self,
        scheduler: &mut Scheduler,
        self_key: Key<TonicClientEvent>,
    ) -> Result<(), TonicError> {
        // Find target endpoint
        let target_endpoint = self
            .endpoint_registry
            .get_endpoint_for_service(&self.service_name)
            .ok_or_else(|| TonicError::ServiceNotFound(self.service_name.clone()))?;

        // Generate request payload
        let request_data = (self.request_generator)(self.request_counter);
        let request_payload = self.codec.encode(&request_data)?;

        // Create RPC request
        let method = MethodDescriptor::new(self.service_name.clone(), self.method_name.clone());
        let rpc_request =
            RpcRequest::new(method, request_payload).with_timeout(self.request_timeout);

        // Generate correlation ID
        self.request_counter += 1;
        let correlation_id = format!("{}-req-{}", self.name, self.request_counter);

        // Serialize request for transport
        let transport_payload = utils::serialize_rpc_request(&rpc_request);

        // Create transport message
        let transport_message = TransportMessage::new(
            self.request_counter,
            self.endpoint_id,
            target_endpoint.id,
            transport_payload,
            scheduler.time(),
            MessageType::UnaryRequest,
        )
        .with_correlation_id(correlation_id.clone());

        // Send through transport
        scheduler.schedule_now(
            self.transport_key,
            TransportEvent::SendMessage {
                message: transport_message,
            },
        );

        // Track pending request
        self.pending_requests
            .insert(correlation_id.clone(), scheduler.time());
        self.requests_sent += 1;

        // Schedule timeout
        scheduler.schedule(
            SimTime::from_duration(self.request_timeout),
            self_key,
            TonicClientEvent::RequestTimeout { correlation_id },
        );

        println!(
            "[{}] [{}] Sent request {} to {} (total sent: {})",
            scheduler.time().as_duration().as_millis(),
            self.name,
            self.request_counter,
            target_endpoint.service_name,
            self.requests_sent
        );

        Ok(())
    }

    /// Handle response from server
    fn handle_response(
        &mut self,
        response: RpcResponse,
        correlation_id: String,
        scheduler: &mut Scheduler,
    ) {
        if let Some(sent_time) = self.pending_requests.remove(&correlation_id) {
            self.responses_received += 1;
            let latency = scheduler.time().duration_since(sent_time);

            match response.status {
                RpcStatus::Ok => {
                    self.successful_responses += 1;
                    println!(
                        "[{}] [{}] Received successful response for {} (latency: {}ms)",
                        scheduler.time().as_duration().as_millis(),
                        self.name,
                        correlation_id,
                        latency.as_millis()
                    );
                }
                RpcStatus::Error { code, message } => {
                    self.failed_responses += 1;
                    println!(
                        "[{}] [{}] Received error response for {}: {:?} - {} (latency: {}ms)",
                        scheduler.time().as_duration().as_millis(),
                        self.name,
                        correlation_id,
                        code,
                        message,
                        latency.as_millis()
                    );
                }
            }
        }
    }

    /// Handle request timeout
    fn handle_timeout(&mut self, correlation_id: String, scheduler: &mut Scheduler) {
        if self.pending_requests.remove(&correlation_id).is_some() {
            self.requests_timed_out += 1;
            println!(
                "[{}] [{}] Request {} timed out (total timeouts: {})",
                scheduler.time().as_duration().as_millis(),
                self.name,
                correlation_id,
                self.requests_timed_out
            );
        }
    }

    /// Schedule next periodic request
    fn schedule_next_request(&self, scheduler: &mut Scheduler, self_key: Key<TonicClientEvent>) {
        scheduler.schedule(
            SimTime::from_duration(self.request_interval),
            self_key,
            TonicClientEvent::SendPeriodicRequest,
        );
    }

    /// Get client statistics
    pub fn stats(&self) -> ClientStats {
        ClientStats {
            requests_sent: self.requests_sent,
            responses_received: self.responses_received,
            requests_timed_out: self.requests_timed_out,
            successful_responses: self.successful_responses,
            failed_responses: self.failed_responses,
            pending_requests: self.pending_requests.len() as u64,
        }
    }
}

/// Client statistics
#[derive(Debug, Clone)]
pub struct ClientStats {
    pub requests_sent: u64,
    pub responses_received: u64,
    pub requests_timed_out: u64,
    pub successful_responses: u64,
    pub failed_responses: u64,
    pub pending_requests: u64,
}

impl<T> Component for DesTonicClient<T>
where
    T: Send + Sync + 'static,
{
    type Event = TonicClientEvent;

    fn process_event(
        &mut self,
        self_id: Key<Self::Event>,
        event: &Self::Event,
        scheduler: &mut Scheduler,
    ) {
        match event {
            TonicClientEvent::SendPeriodicRequest => {
                if let Err(e) = self.send_request(scheduler, self_id) {
                    eprintln!("[{}] Failed to send request: {}", self.name, e);
                }
                // Schedule next request
                self.schedule_next_request(scheduler, self_id);
            }
            TonicClientEvent::ResponseReceived {
                response,
                correlation_id,
            } => {
                self.handle_response(response.clone(), correlation_id.clone(), scheduler);
            }
            TonicClientEvent::RequestTimeout { correlation_id } => {
                self.handle_timeout(correlation_id.clone(), scheduler);
            }
        }
    }
}

/// Builder for DES tonic clients
pub struct DesTonicClientBuilder<T> {
    name: Option<String>,
    service_name: Option<String>,
    method_name: Option<String>,
    transport_key: Option<Key<TransportEvent>>,
    endpoint_registry: Option<SharedEndpointRegistry>,
    codec: Option<Box<dyn RpcCodec<T>>>,
    request_generator: Option<Box<dyn Fn(u64) -> T + Send + Sync>>,
    interval: Duration,
    timeout: Duration,
}

impl<T> DesTonicClientBuilder<T>
where
    T: Send + Sync + 'static,
{
    /// Create a new client builder
    pub fn new() -> Self {
        Self {
            name: None,
            service_name: None,
            method_name: None,
            transport_key: None,
            endpoint_registry: None,
            codec: None,
            request_generator: None,
            interval: Duration::from_secs(1),
            timeout: Duration::from_secs(5),
        }
    }

    /// Set client name
    pub fn name(mut self, name: String) -> Self {
        self.name = Some(name);
        self
    }

    /// Set service name
    pub fn service_name(mut self, service_name: String) -> Self {
        self.service_name = Some(service_name);
        self
    }

    /// Set method name
    pub fn method_name(mut self, method_name: String) -> Self {
        self.method_name = Some(method_name);
        self
    }

    /// Set transport key
    pub fn transport_key(mut self, transport_key: Key<TransportEvent>) -> Self {
        self.transport_key = Some(transport_key);
        self
    }

    /// Set endpoint registry
    pub fn endpoint_registry(mut self, endpoint_registry: SharedEndpointRegistry) -> Self {
        self.endpoint_registry = Some(endpoint_registry);
        self
    }

    /// Set codec
    pub fn codec(mut self, codec: Box<dyn RpcCodec<T>>) -> Self {
        self.codec = Some(codec);
        self
    }

    /// Set request generator
    pub fn request_generator(mut self, generator: Box<dyn Fn(u64) -> T + Send + Sync>) -> Self {
        self.request_generator = Some(generator);
        self
    }

    /// Set request interval
    pub fn interval(mut self, interval: Duration) -> Self {
        self.interval = interval;
        self
    }

    /// Set request timeout
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Build the client
    pub fn build(self) -> Result<DesTonicClient<T>, TonicError> {
        let name = self
            .name
            .ok_or_else(|| TonicError::Internal("Name is required".to_string()))?;
        let service_name = self
            .service_name
            .ok_or_else(|| TonicError::Internal("Service name is required".to_string()))?;
        let method_name = self
            .method_name
            .ok_or_else(|| TonicError::Internal("Method name is required".to_string()))?;
        let transport_key = self
            .transport_key
            .ok_or_else(|| TonicError::Internal("Transport key is required".to_string()))?;
        let endpoint_registry = self
            .endpoint_registry
            .ok_or_else(|| TonicError::Internal("Endpoint registry is required".to_string()))?;
        let codec = self
            .codec
            .ok_or_else(|| TonicError::Internal("Codec is required".to_string()))?;
        let request_generator = self
            .request_generator
            .ok_or_else(|| TonicError::Internal("Request generator is required".to_string()))?;

        Ok(DesTonicClient::new(
            name,
            service_name,
            method_name,
            transport_key,
            endpoint_registry,
            codec,
            request_generator,
        )
        .with_interval(self.interval)
        .with_timeout(self.timeout))
    }
}

impl<T> Default for DesTonicClientBuilder<T>
where
    T: Send + Sync + 'static,
{
    fn default() -> Self {
        Self::new()
    }
}
