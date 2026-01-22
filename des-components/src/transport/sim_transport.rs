//! Simulated transport implementation for RPC communication
//!
//! This module provides the main SimTransport component that orchestrates
//! message delivery through the simulated network using configurable network models.

use crate::transport::{
    endpoint_registry::SharedEndpointRegistry, EndpointId, MessageType, NetworkModel,
    TransportEvent, TransportMessage,
};
use des_core::{Component, Key, Scheduler, SchedulerHandle, SimTime};
use std::collections::HashMap;

/// Main transport component for simulated RPC communication
pub struct SimTransport {
    /// Network model for calculating latency and packet loss
    network_model: Box<dyn NetworkModel>,
    /// Endpoint registry for service discovery
    endpoint_registry: SharedEndpointRegistry,
    /// Message ID counter
    next_message_id: u64,
    /// Active message handlers (endpoint -> component key)
    message_handlers: HashMap<EndpointId, Key<TransportEvent>>,
    /// Transport statistics
    stats: TransportStats,
}

/// Statistics for transport operations
#[derive(Debug, Clone, Default)]
pub struct TransportStats {
    /// Total messages sent
    pub messages_sent: u64,
    /// Total messages delivered
    pub messages_delivered: u64,
    /// Total messages dropped
    pub messages_dropped: u64,
    /// Total bytes sent
    pub bytes_sent: u64,
    /// Total bytes delivered
    pub bytes_delivered: u64,
}

impl SimTransport {
    /// Create a new simulated transport
    pub fn new(network_model: Box<dyn NetworkModel>) -> Self {
        Self {
            network_model,
            endpoint_registry: SharedEndpointRegistry::new(),
            next_message_id: 1,
            message_handlers: HashMap::new(),
            stats: TransportStats::default(),
        }
    }

    /// Get a reference to the endpoint registry
    pub fn endpoint_registry(&self) -> &SharedEndpointRegistry {
        &self.endpoint_registry
    }

    /// Register a message handler for an endpoint
    pub fn register_handler(&mut self, endpoint: EndpointId, handler: Key<TransportEvent>) {
        self.message_handlers.insert(endpoint, handler);
    }

    /// Unregister a message handler
    pub fn unregister_handler(&mut self, endpoint: EndpointId) {
        self.message_handlers.remove(&endpoint);
    }

    /// Send a message through the transport.
    ///
    /// This is a convenience wrapper that schedules a `TransportEvent::SendMessage` to be
    /// processed by this `SimTransport` component.
    pub fn send_message(
        &mut self,
        source: EndpointId,
        destination: EndpointId,
        payload: Vec<u8>,
        message_type: MessageType,
        scheduler_handle: &SchedulerHandle,
        self_key: Key<TransportEvent>,
    ) -> Result<u64, String> {
        let message_id = self.next_message_id;
        self.next_message_id += 1;

        let message = TransportMessage::new(
            message_id,
            source,
            destination,
            payload,
            scheduler_handle.time(),
            message_type,
        );

        scheduler_handle.schedule_now(self_key, TransportEvent::SendMessage { message });
        Ok(message_id)
    }

    /// Send a message with correlation ID for request/response matching.
    pub fn send_message_with_correlation(
        &mut self,
        source: EndpointId,
        destination: EndpointId,
        payload: Vec<u8>,
        message_type: MessageType,
        correlation_id: String,
        scheduler_handle: &SchedulerHandle,
        self_key: Key<TransportEvent>,
    ) -> Result<u64, String> {
        let message_id = self.next_message_id;
        self.next_message_id += 1;

        let message = TransportMessage::new(
            message_id,
            source,
            destination,
            payload,
            scheduler_handle.time(),
            message_type,
        )
        .with_correlation_id(correlation_id);

        scheduler_handle.schedule_now(self_key, TransportEvent::SendMessage { message });
        Ok(message_id)
    }

    /// Handle message delivery
    fn handle_message_delivered(&mut self, message: TransportMessage, scheduler: &mut Scheduler) {
        self.stats.messages_delivered += 1;
        self.stats.bytes_delivered += message.size() as u64;

        let destination = message.destination;

        // Forward to registered handler if available
        if let Some(&handler_key) = self.message_handlers.get(&destination) {
            scheduler.schedule_now(handler_key, TransportEvent::MessageDelivered { message });
        } else {
            // No handler registered - message is lost
            println!(
                "Warning: No handler registered for endpoint {}, message {} dropped",
                destination, message.id
            );
        }
    }

    /// Handle message drop
    fn handle_message_dropped(&mut self, message: TransportMessage, reason: String) {
        println!(
            "Message {} from {} to {} dropped: {}",
            message.id, message.source, message.destination, reason
        );
    }

    /// Get transport statistics
    pub fn stats(&self) -> &TransportStats {
        &self.stats
    }

    /// Reset transport statistics
    pub fn reset_stats(&mut self) {
        self.stats = TransportStats::default();
    }

    /// Reset network model state
    pub fn reset_network_model(&mut self) {
        self.network_model.reset();
    }
}

impl Component for SimTransport {
    type Event = TransportEvent;

    fn process_event(
        &mut self,
        self_id: Key<Self::Event>,
        event: &Self::Event,
        scheduler: &mut Scheduler,
    ) {
        match event {
            TransportEvent::SendMessage { message } => {
                // Handle message send request - assign message ID (if needed) and apply network simulation
                let mut message_with_id = message.clone();

                if message_with_id.id == 0 {
                    message_with_id.id = self.next_message_id;
                    self.next_message_id += 1;
                } else if message_with_id.id >= self.next_message_id {
                    self.next_message_id = message_with_id.id + 1;
                }

                // The message enters the network when this event is processed.
                message_with_id.sent_at = scheduler.time();

                // Apply network simulation directly without using scheduler handle
                self.stats.messages_sent += 1;
                self.stats.bytes_sent += message_with_id.size() as u64;

                // Check if message should be dropped
                if self.network_model.should_drop_message(
                    message_with_id.source,
                    message_with_id.destination,
                    &message_with_id,
                ) {
                    self.stats.messages_dropped += 1;
                    scheduler.schedule_now(
                        self_id,
                        TransportEvent::MessageDropped {
                            message: message_with_id,
                            reason: "Network packet loss".to_string(),
                        },
                    );
                } else {
                    // Calculate total delivery time (latency + bandwidth delay)
                    let latency = self.network_model.calculate_latency(
                        message_with_id.source,
                        message_with_id.destination,
                        &message_with_id,
                    );
                    

                    // Schedule delivery
                    scheduler.schedule(
                        SimTime::from_duration(latency),
                        self_id,
                        TransportEvent::MessageDelivered {
                            message: message_with_id,
                        },
                    );
                }
            }
            TransportEvent::MessageDelivered { message } => {
                self.handle_message_delivered(message.clone(), scheduler);
            }
            TransportEvent::MessageDropped { message, reason } => {
                self.handle_message_dropped(message.clone(), reason.clone());
            }
        }
    }
}

/// Builder for creating SimTransport instances
pub struct SimTransportBuilder {
    network_model: Option<Box<dyn NetworkModel>>,
}

impl SimTransportBuilder {
    /// Create a new transport builder
    pub fn new() -> Self {
        Self {
            network_model: None,
        }
    }

    /// Set the network model
    pub fn network_model(mut self, model: Box<dyn NetworkModel>) -> Self {
        self.network_model = Some(model);
        self
    }

    /// Build the transport
    pub fn build(self) -> Result<SimTransport, String> {
        let network_model = self
            .network_model
            .ok_or_else(|| "Network model is required".to_string())?;

        Ok(SimTransport::new(network_model))
    }
}

impl Default for SimTransportBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::{endpoint_registry::EndpointInfo, SimpleNetworkModel};
    use des_core::Simulation;
    use std::time::Duration;

    #[test]
    fn test_sim_transport_creation() {
        let network_model = Box::new(SimpleNetworkModel::with_seed(
            Duration::from_millis(100),
            0.0, // No packet loss for this test
            42,
        ));

        let transport = SimTransport::new(network_model);

        assert_eq!(transport.stats().messages_sent, 0);
        assert_eq!(transport.stats().messages_delivered, 0);
        assert_eq!(transport.stats().messages_dropped, 0);
    }

    #[test]
    fn test_message_sending() {
        let mut sim = Simulation::default();

        let network_model = Box::new(SimpleNetworkModel::with_seed(
            Duration::from_millis(50),
            0.0, // No packet loss
            123,
        ));

        let transport = SimTransport::new(network_model);
        let transport_key = sim.add_component(transport);

        let endpoint1 = EndpointId::new("service1".to_string());
        let endpoint2 = EndpointId::new("service2".to_string());

        // Send a message
        let scheduler_handle = sim.scheduler_handle();
        let message_id = {
            let transport_ref = sim
                .get_component_mut::<TransportEvent, SimTransport>(transport_key)
                .unwrap();
            transport_ref
                .send_message(
                    endpoint1,
                    endpoint2,
                    vec![1, 2, 3, 4],
                    MessageType::UnaryRequest,
                    &scheduler_handle,
                    transport_key,
                )
                .unwrap()
        };

        assert_eq!(message_id, 1);

        // Stats update when the send event is processed.
        {
            let transport_ref = sim
                .get_component_mut::<TransportEvent, SimTransport>(transport_key)
                .unwrap();
            assert_eq!(transport_ref.stats().messages_sent, 0);
        }

        // Run simulation to process the message
        for _ in 0..100 {
            if !sim.step() {
                break;
            }
        }

        // Check that message was processed (though no handler was registered)
        let transport_ref = sim
            .get_component_mut::<TransportEvent, SimTransport>(transport_key)
            .unwrap();
        assert_eq!(transport_ref.stats().messages_delivered, 1);
        assert_eq!(transport_ref.stats().bytes_delivered, 4);
    }

    #[test]
    fn test_packet_loss() {
        let mut sim = Simulation::default();

        let network_model = Box::new(SimpleNetworkModel::with_seed(
            Duration::from_millis(50),
            1.0, // 100% packet loss
            456,
        ));

        let transport = SimTransport::new(network_model);
        let transport_key = sim.add_component(transport);

        let endpoint1 = EndpointId::new("service1".to_string());
        let endpoint2 = EndpointId::new("service2".to_string());

        // Send a message that should be dropped
        let scheduler_handle = sim.scheduler_handle();
        {
            let transport_ref = sim
                .get_component_mut::<TransportEvent, SimTransport>(transport_key)
                .unwrap();
            transport_ref
                .send_message(
                    endpoint1,
                    endpoint2,
                    vec![1, 2, 3, 4],
                    MessageType::UnaryRequest,
                    &scheduler_handle,
                    transport_key,
                )
                .unwrap();

            assert_eq!(transport_ref.stats().messages_sent, 0);
        }

        // Run simulation
        for _ in 0..100 {
            if !sim.step() {
                break;
            }
        }

        // Check that message was dropped
        let transport_ref = sim
            .get_component_mut::<TransportEvent, SimTransport>(transport_key)
            .unwrap();
        assert_eq!(transport_ref.stats().messages_dropped, 1);
        assert_eq!(transport_ref.stats().messages_delivered, 0);
    }

    #[test]
    fn test_transport_builder() {
        let network_model = Box::new(SimpleNetworkModel::new(Duration::from_millis(100), 0.1));

        let transport = SimTransportBuilder::new()
            .network_model(network_model)
            .build()
            .unwrap();

        assert_eq!(transport.stats().messages_sent, 0);
    }

    #[test]
    fn test_endpoint_registry_integration() {
        let network_model = Box::new(SimpleNetworkModel::new(Duration::from_millis(100), 0.0));

        let transport = SimTransport::new(network_model);

        // Register an endpoint
        let endpoint_info = EndpointInfo::new("user-service".to_string(), "instance-1".to_string());
        transport
            .endpoint_registry()
            .register_endpoint(endpoint_info.clone())
            .unwrap();

        // Find the endpoint
        let found_endpoints = transport.endpoint_registry().find_endpoints("user-service");
        assert_eq!(found_endpoints.len(), 1);
        assert_eq!(found_endpoints[0].service_name, "user-service");
    }
}
