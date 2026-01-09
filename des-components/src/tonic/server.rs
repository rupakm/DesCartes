//! Tonic-compatible RPC server implementation for discrete event simulation

use crate::tonic::{
    MethodDescriptor, RpcCodec, RpcEvent, RpcRequest, RpcResponse, RpcService, RpcStatus,
    RpcStatusCode, TonicError, TonicResult, utils,
};
use crate::transport::{EndpointId, EndpointInfo, MessageType, SharedEndpointRegistry, TransportEvent};
use des_core::{Component, Key, Scheduler, SchedulerHandle, SimTime};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::oneshot;

/// Tonic-compatible RPC server for discrete event simulation
pub struct SimTonicServer {
    /// Server endpoint ID
    endpoint_id: EndpointId,
    /// Service name
    service_name: String,
    /// Instance name
    instance_name: String,
    /// Transport component key
    transport_key: Key<TransportEvent>,
    /// Endpoint registry
    endpoint_registry: SharedEndpointRegistry,
    /// Registered services (method_name -> service)
    services: HashMap<String, Box<dyn RpcService>>,
    /// Request processing timeout
    processing_timeout: Duration,
}

impl SimTonicServer {
    /// Create a new tonic server
    pub fn new(
        service_name: String,
        instance_name: String,
        transport_key: Key<TransportEvent>,
        endpoint_registry: SharedEndpointRegistry,
    ) -> Self {
        let endpoint_id = EndpointId::new(format!("{}:{}", service_name, instance_name));
        
        Self {
            endpoint_id,
            service_name,
            instance_name,
            transport_key,
            endpoint_registry,
            services: HashMap::new(),
            processing_timeout: Duration::from_secs(30),
        }
    }

    /// Set processing timeout
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.processing_timeout = timeout;
        self
    }

    /// Register a service implementation
    pub fn add_service<S: RpcService + 'static>(mut self, service: S) -> Self {
        let service_name = service.service_name().to_string();
        self.services.insert(service_name, Box::new(service));
        self
    }

    /// Start the server (register with endpoint registry)
    pub fn start(&self) -> TonicResult<()> {
        let endpoint_info = EndpointInfo::new(self.service_name.clone(), self.instance_name.clone())
            .with_metadata("type".to_string(), "grpc".to_string())
            .with_metadata("endpoint_id".to_string(), self.endpoint_id.id().to_string());

        self.endpoint_registry
            .register_endpoint(endpoint_info)
            .map_err(|e| TonicError::Internal(format!("Failed to register endpoint: {}", e)))?;

        Ok(())
    }

    /// Stop the server (unregister from endpoint registry)
    pub fn stop(&self) -> TonicResult<()> {
        self.endpoint_registry
            .unregister_endpoint(self.endpoint_id)
            .map_err(|e| TonicError::Internal(format!("Failed to unregister endpoint: {}", e)))?;

        Ok(())
    }

    /// Handle incoming RPC request
    pub fn handle_request(
        &mut self,
        request: RpcRequest,
        response_sender: Arc<Mutex<Option<oneshot::Sender<RpcResponse>>>>,
        client_endpoint: EndpointId,
        scheduler_handle: &SchedulerHandle,
    ) -> TonicResult<()> {
        // Find service for this method
        let service = self.services.get_mut(&request.method.service)
            .ok_or_else(|| TonicError::ServiceNotFound(request.method.service.clone()))?;

        // Check if method is supported
        if !service.supported_methods().contains(&request.method.method) {
            let error_response = RpcResponse::error(
                RpcStatusCode::Unimplemented,
                format!("Method {} not implemented", request.method.method),
            );
            self.send_response(error_response, response_sender, client_endpoint, scheduler_handle)?;
            return Ok(());
        }

        // Process request
        match service.handle_request(request, scheduler_handle) {
            Ok(response) => {
                self.send_response(response, response_sender, client_endpoint, scheduler_handle)?;
            }
            Err(error) => {
                let error_response = match error {
                    TonicError::RpcFailed { code, message } => {
                        RpcResponse::error(code, message)
                    }
                    _ => {
                        RpcResponse::error(RpcStatusCode::Internal, error.to_string())
                    }
                };
                self.send_response(error_response, response_sender, client_endpoint, scheduler_handle)?;
            }
        }

        Ok(())
    }

    /// Send response back to client
    fn send_response(
        &self,
        response: RpcResponse,
        response_sender: Arc<Mutex<Option<oneshot::Sender<RpcResponse>>>>,
        client_endpoint: EndpointId,
        scheduler_handle: &SchedulerHandle,
    ) -> TonicResult<()> {
        // Send response through oneshot channel (for local delivery)
        {
            let mut sender_opt = response_sender.lock().unwrap();
            if let Some(sender) = sender_opt.take() {
                let _ = sender.send(response.clone());
            }
        }

        // Also send through transport (for network simulation)
        let transport_payload = utils::serialize_rpc_response(&response);
        
        // Schedule transport message
        scheduler_handle.schedule_now(
            self.transport_key,
            TransportEvent::MessageDelivered {
                message: crate::transport::TransportMessage::new(
                    0, // message ID will be assigned by transport
                    self.endpoint_id,
                    client_endpoint,
                    transport_payload,
                    scheduler_handle.time(),
                    MessageType::UnaryResponse,
                ),
                destination: client_endpoint,
            },
        );

        Ok(())
    }

    /// Get server endpoint ID
    pub fn endpoint_id(&self) -> EndpointId {
        self.endpoint_id
    }

    /// Get service name
    pub fn service_name(&self) -> &str {
        &self.service_name
    }

    /// List registered services
    pub fn list_services(&self) -> Vec<String> {
        self.services.keys().cloned().collect()
    }
}

/// Component wrapper for SimTonicServer to handle RPC events
pub struct TonicServerComponent {
    server: SimTonicServer,
}

impl TonicServerComponent {
    /// Create a new server component
    pub fn new(server: SimTonicServer) -> Self {
        Self { server }
    }

    /// Get reference to the underlying server
    pub fn server(&self) -> &SimTonicServer {
        &self.server
    }

    /// Get mutable reference to the underlying server
    pub fn server_mut(&mut self) -> &mut SimTonicServer {
        &mut self.server
    }
}

impl Component for TonicServerComponent {
    type Event = RpcEvent;

    fn process_event(
        &mut self,
        _self_id: Key<Self::Event>,
        event: &Self::Event,
        scheduler: &mut Scheduler,
    ) {        
        match event {
            RpcEvent::RequestReceived {
                request,
                response_sender,
                client_endpoint,
            } => {
                // For now, we'll handle the request synchronously
                // In a real implementation, this would be more sophisticated
                let mut sim = des_core::Simulation::default();
                let scheduler_handle = sim.scheduler_handle();
                if let Err(e) = self.server.handle_request(
                    request.clone(),
                    response_sender.clone(),
                    *client_endpoint,
                    &scheduler_handle,
                ) {
                    eprintln!("Error handling RPC request: {}", e);
                }
            }
            _ => {
                // Ignore other events
            }
        }
    }
}

/// Builder for creating tonic servers
pub struct TonicServerBuilder {
    service_name: Option<String>,
    instance_name: Option<String>,
    transport_key: Option<Key<TransportEvent>>,
    endpoint_registry: Option<SharedEndpointRegistry>,
    services: Vec<Box<dyn RpcService>>,
    timeout: Duration,
}

impl TonicServerBuilder {
    /// Create a new server builder
    pub fn new() -> Self {
        Self {
            service_name: None,
            instance_name: None,
            transport_key: None,
            endpoint_registry: None,
            services: Vec::new(),
            timeout: Duration::from_secs(30),
        }
    }

    /// Set the service name
    pub fn service_name(mut self, service_name: String) -> Self {
        self.service_name = Some(service_name);
        self
    }

    /// Set the instance name
    pub fn instance_name(mut self, instance_name: String) -> Self {
        self.instance_name = Some(instance_name);
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

    /// Add a service implementation
    pub fn add_service<S: RpcService + 'static>(mut self, service: S) -> Self {
        self.services.push(Box::new(service));
        self
    }

    /// Set the processing timeout
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Build the server
    pub fn build(self) -> TonicResult<SimTonicServer> {
        let service_name = self.service_name
            .ok_or_else(|| TonicError::Internal("Service name is required".to_string()))?;
        let instance_name = self.instance_name
            .unwrap_or_else(|| "default".to_string());
        let transport_key = self.transport_key
            .ok_or_else(|| TonicError::Internal("Transport key is required".to_string()))?;
        let endpoint_registry = self.endpoint_registry
            .ok_or_else(|| TonicError::Internal("Endpoint registry is required".to_string()))?;

        let mut server = SimTonicServer::new(service_name, instance_name, transport_key, endpoint_registry)
            .with_timeout(self.timeout);

        // Add all services
        for service in self.services {
            let service_name = service.service_name().to_string();
            server.services.insert(service_name, service);
        }

        Ok(server)
    }
}

impl Default for TonicServerBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Example service implementation
#[cfg(test)]
pub struct EchoService;

#[cfg(test)]
impl RpcService for EchoService {
    fn handle_request(
        &mut self,
        request: RpcRequest,
        _scheduler_handle: &SchedulerHandle,
    ) -> Result<RpcResponse, TonicError> {
        match request.method.method.as_str() {
            "Echo" => {
                // Echo the request payload back as response
                Ok(RpcResponse::success(request.payload))
            }
            _ => {
                Err(TonicError::MethodNotFound {
                    service: request.method.service,
                    method: request.method.method,
                })
            }
        }
    }

    fn service_name(&self) -> &str {
        "echo.EchoService"
    }

    fn supported_methods(&self) -> Vec<String> {
        vec!["Echo".to_string()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::{SimpleNetworkModel, SimTransport};
    use des_core::Simulation;

    #[test]
    fn test_server_creation() {
        let mut sim = Simulation::default();
        
        // Create transport
        let network_model = Box::new(SimpleNetworkModel::new(
            Duration::from_millis(10),
            0.0,
        ));
        let transport = SimTransport::new(network_model);
        let transport_key = sim.add_component(transport);

        // Create server
        let server = TonicServerBuilder::new()
            .service_name("echo.EchoService".to_string())
            .instance_name("test-instance".to_string())
            .transport_key(transport_key)
            .endpoint_registry(SharedEndpointRegistry::new())
            .add_service(EchoService)
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap();

        assert_eq!(server.service_name(), "echo.EchoService");
        assert_eq!(server.list_services(), vec!["echo.EchoService"]);
    }

    #[test]
    fn test_server_builder_validation() {
        // Missing service name
        let result = TonicServerBuilder::new()
            .build();
        assert!(result.is_err());

        // Missing transport key
        let result = TonicServerBuilder::new()
            .service_name("test.Service".to_string())
            .build();
        assert!(result.is_err());
    }

    #[test]
    fn test_echo_service() {
        let mut service = EchoService;
        let mut sim = des_core::Simulation::default();
        let scheduler_handle = sim.scheduler_handle();

        let method = MethodDescriptor::new("echo.EchoService".to_string(), "Echo".to_string());
        let request = RpcRequest::new(method, vec![1, 2, 3, 4]);

        let response = service.handle_request(request, &scheduler_handle).unwrap();
        assert!(response.is_success());
        assert_eq!(response.payload, vec![1, 2, 3, 4]);
    }
}