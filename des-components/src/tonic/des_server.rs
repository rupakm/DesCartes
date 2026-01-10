//! DES-integrated tonic server component that handles RPC requests through transport

use crate::tonic::{utils, RpcResponse, RpcService, RpcStatusCode, TonicError};
use crate::transport::{
    EndpointId, EndpointInfo, MessageType, SharedEndpointRegistry, TransportEvent, TransportMessage,
};
use des_core::{Component, Key, Scheduler, SimTime};
use std::collections::HashMap;
use std::time::Duration;

/// Events for the DES tonic server
#[derive(Debug, Clone)]
pub enum TonicServerEvent {
    /// RPC request received through transport
    RequestReceived { message: TransportMessage },
    /// Processing completed, ready to send response
    ProcessingComplete {
        response: RpcResponse,
        original_message: TransportMessage,
    },
}

/// DES component that wraps a tonic server and handles RPC requests
pub struct DesTonicServer {
    /// Server name for identification
    pub name: String,
    /// Server endpoint ID
    pub endpoint_id: EndpointId,
    /// Service name
    pub service_name: String,
    /// Instance name
    pub instance_name: String,
    /// Transport component key
    pub transport_key: Key<TransportEvent>,
    /// Endpoint registry
    pub endpoint_registry: SharedEndpointRegistry,
    /// Registered services (service_name -> service)
    pub services: HashMap<String, Box<dyn RpcService>>,
    /// Request processing timeout
    pub processing_timeout: Duration,
    /// Statistics
    pub requests_received: u64,
    pub requests_processed: u64,
    pub requests_failed: u64,
    pub responses_sent: u64,
}

impl DesTonicServer {
    /// Create a new DES tonic server
    pub fn new(
        name: String,
        service_name: String,
        instance_name: String,
        transport_key: Key<TransportEvent>,
        endpoint_registry: SharedEndpointRegistry,
    ) -> Self {
        let endpoint_id = EndpointId::new(format!("{}:{}", service_name, instance_name));

        Self {
            name,
            endpoint_id,
            service_name,
            instance_name,
            transport_key,
            endpoint_registry,
            services: HashMap::new(),
            processing_timeout: Duration::from_secs(30),
            requests_received: 0,
            requests_processed: 0,
            requests_failed: 0,
            responses_sent: 0,
        }
    }

    /// Set processing timeout
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.processing_timeout = timeout;
        self
    }

    /// Add a service implementation
    pub fn add_service<S: RpcService + 'static>(mut self, service: S) -> Self {
        let service_name = service.service_name().to_string();
        self.services.insert(service_name, Box::new(service));
        self
    }

    /// Start the server (register with endpoint registry)
    pub fn start(&self) -> Result<(), TonicError> {
        let endpoint_info =
            EndpointInfo::new(self.service_name.clone(), self.instance_name.clone())
                .with_metadata("type".to_string(), "grpc".to_string())
                .with_metadata("endpoint_id".to_string(), self.endpoint_id.id().to_string());

        self.endpoint_registry
            .register_endpoint(endpoint_info)
            .map_err(|e| TonicError::Internal(format!("Failed to register endpoint: {}", e)))?;

        println!(
            "[{}] Server {} registered and listening on endpoint {}",
            0, // Will be updated when we have scheduler time
            self.name,
            self.endpoint_id
        );

        Ok(())
    }

    /// Handle incoming RPC request from transport
    fn handle_transport_message(
        &mut self,
        message: TransportMessage,
        scheduler: &mut Scheduler,
        self_key: Key<TonicServerEvent>,
    ) -> Result<(), TonicError> {
        self.requests_received += 1;

        println!(
            "[{}] [{}] Received request {} from endpoint {} (total received: {})",
            scheduler.time().as_duration().as_millis(),
            self.name,
            message.id,
            message.source,
            self.requests_received
        );

        // Deserialize RPC request
        let rpc_request = utils::deserialize_rpc_request(&message.payload)?;

        // Find service for this method
        let service = self
            .services
            .get_mut(&rpc_request.method.service)
            .ok_or_else(|| TonicError::ServiceNotFound(rpc_request.method.service.clone()))?;

        // Check if method is supported
        if !service
            .supported_methods()
            .contains(&rpc_request.method.method)
        {
            let error_response = RpcResponse::error(
                RpcStatusCode::Unimplemented,
                format!("Method {} not implemented", rpc_request.method.method),
            );
            // Schedule processing completion after delay
            scheduler.schedule(
                SimTime::from_duration(std::time::Duration::from_millis(100)),
                self_key,
                TonicServerEvent::ProcessingComplete {
                    response: error_response,
                    original_message: message,
                },
            );
            return Ok(());
        }

        // Process request
        let scheduler_handle = scheduler.handle();
        match service.handle_request(rpc_request, &scheduler_handle) {
            Ok(response) => {
                self.requests_processed += 1;
                // Schedule processing completion after delay
                scheduler.schedule(
                    SimTime::from_duration(std::time::Duration::from_millis(100)),
                    self_key,
                    TonicServerEvent::ProcessingComplete {
                        response,
                        original_message: message,
                    },
                );
            }
            Err(error) => {
                self.requests_failed += 1;
                let error_response = match error {
                    TonicError::RpcFailed { code, message: msg } => RpcResponse::error(code, msg),
                    _ => RpcResponse::error(RpcStatusCode::Internal, error.to_string()),
                };
                // Schedule processing completion after delay
                scheduler.schedule(
                    SimTime::from_duration(std::time::Duration::from_millis(100)),
                    self_key,
                    TonicServerEvent::ProcessingComplete {
                        response: error_response,
                        original_message: message,
                    },
                );
            }
        }

        Ok(())
    }

    /// Send response back to client through transport
    fn send_response(
        &mut self,
        response: RpcResponse,
        original_message: TransportMessage,
        scheduler: &mut Scheduler,
    ) -> Result<(), TonicError> {
        // Serialize response
        let response_payload = utils::serialize_rpc_response(&response);

        // Create response message
        let response_message = TransportMessage::new(
            original_message.id + 1000000, // Offset to avoid ID conflicts
            self.endpoint_id,
            original_message.source,
            response_payload,
            scheduler.time(),
            MessageType::UnaryResponse,
        );

        // Add correlation ID if present
        let response_message = if let Some(correlation_id) = &original_message.correlation_id {
            response_message.with_correlation_id(correlation_id.clone())
        } else {
            response_message
        };

        // Send through transport
        scheduler.schedule_now(
            self.transport_key,
            TransportEvent::SendMessage {
                message: response_message,
            },
        );

        self.responses_sent += 1;

        println!(
            "[{}] [{}] Sent response for request {} to endpoint {} (total sent: {})",
            scheduler.time().as_duration().as_millis(),
            self.name,
            original_message.id,
            original_message.source,
            self.responses_sent
        );

        Ok(())
    }

    /// Get server statistics
    pub fn stats(&self) -> ServerStats {
        ServerStats {
            requests_received: self.requests_received,
            requests_processed: self.requests_processed,
            requests_failed: self.requests_failed,
            responses_sent: self.responses_sent,
        }
    }

    /// Get server endpoint ID
    pub fn endpoint_id(&self) -> EndpointId {
        self.endpoint_id
    }

    /// List registered services
    pub fn list_services(&self) -> Vec<String> {
        self.services.keys().cloned().collect()
    }
}

/// Server statistics
#[derive(Debug, Clone)]
pub struct ServerStats {
    pub requests_received: u64,
    pub requests_processed: u64,
    pub requests_failed: u64,
    pub responses_sent: u64,
}

impl Component for DesTonicServer {
    type Event = TonicServerEvent;

    fn process_event(
        &mut self,
        self_id: Key<Self::Event>,
        event: &Self::Event,
        scheduler: &mut Scheduler,
    ) {
        match event {
            TonicServerEvent::RequestReceived { message } => {
                if let Err(e) = self.handle_transport_message(message.clone(), scheduler, self_id) {
                    eprintln!("[{}] Error handling request: {}", self.name, e);
                }
            }
            TonicServerEvent::ProcessingComplete {
                response,
                original_message,
            } => {
                if let Err(e) =
                    self.send_response(response.clone(), original_message.clone(), scheduler)
                {
                    eprintln!("[{}] Error sending response: {}", self.name, e);
                }
            }
        }
    }
}

/// Builder for DES tonic servers
pub struct DesTonicServerBuilder {
    name: Option<String>,
    service_name: Option<String>,
    instance_name: Option<String>,
    transport_key: Option<Key<TransportEvent>>,
    endpoint_registry: Option<SharedEndpointRegistry>,
    services: Vec<Box<dyn RpcService>>,
    timeout: Duration,
}

impl DesTonicServerBuilder {
    /// Create a new server builder
    pub fn new() -> Self {
        Self {
            name: None,
            service_name: None,
            instance_name: None,
            transport_key: None,
            endpoint_registry: None,
            services: Vec::new(),
            timeout: Duration::from_secs(30),
        }
    }

    /// Set server name
    pub fn name(mut self, name: String) -> Self {
        self.name = Some(name);
        self
    }

    /// Set service name
    pub fn service_name(mut self, service_name: String) -> Self {
        self.service_name = Some(service_name);
        self
    }

    /// Set instance name
    pub fn instance_name(mut self, instance_name: String) -> Self {
        self.instance_name = Some(instance_name);
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

    /// Add a service implementation
    pub fn add_service<S: RpcService + 'static>(mut self, service: S) -> Self {
        self.services.push(Box::new(service));
        self
    }

    /// Set processing timeout
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Build the server
    pub fn build(self) -> Result<DesTonicServer, TonicError> {
        let name = self.name.unwrap_or_else(|| "server".to_string());
        let service_name = self
            .service_name
            .ok_or_else(|| TonicError::Internal("Service name is required".to_string()))?;
        let instance_name = self.instance_name.unwrap_or_else(|| "default".to_string());
        let transport_key = self
            .transport_key
            .ok_or_else(|| TonicError::Internal("Transport key is required".to_string()))?;
        let endpoint_registry = self
            .endpoint_registry
            .ok_or_else(|| TonicError::Internal("Endpoint registry is required".to_string()))?;

        let mut server = DesTonicServer::new(
            name,
            service_name,
            instance_name,
            transport_key,
            endpoint_registry,
        )
        .with_timeout(self.timeout);

        // Add all services
        for service in self.services {
            let service_name = service.service_name().to_string();
            server.services.insert(service_name, service);
        }

        Ok(server)
    }
}

impl Default for DesTonicServerBuilder {
    fn default() -> Self {
        Self::new()
    }
}
