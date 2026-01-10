//! Transport router for routing RPC messages between tonic clients and servers

use crate::tonic::{utils, TonicClientEvent, TonicServerEvent};
use crate::transport::{EndpointId, MessageType, TransportEvent, TransportMessage};
use des_core::{Component, Key, Scheduler};
use std::collections::HashMap;

/// Transport router that routes messages between tonic clients and servers
pub struct TonicTransportRouter {
    /// Map from endpoint ID to client component key
    client_endpoints: HashMap<EndpointId, Key<TonicClientEvent>>,
    /// Map from endpoint ID to server component key
    server_endpoints: HashMap<EndpointId, Key<TonicServerEvent>>,
}

impl TonicTransportRouter {
    /// Create a new transport router
    pub fn new() -> Self {
        Self {
            client_endpoints: HashMap::new(),
            server_endpoints: HashMap::new(),
        }
    }

    /// Register a client endpoint
    pub fn register_client(&mut self, endpoint_id: EndpointId, client_key: Key<TonicClientEvent>) {
        self.client_endpoints.insert(endpoint_id, client_key);
    }

    /// Register a server endpoint
    pub fn register_server(&mut self, endpoint_id: EndpointId, server_key: Key<TonicServerEvent>) {
        self.server_endpoints.insert(endpoint_id, server_key);
    }

    /// Route a transport message to the appropriate component
    fn route_message(&self, message: TransportMessage, scheduler: &mut Scheduler) {
        match message.message_type {
            MessageType::UnaryRequest => {
                // Route to server
                if let Some(&server_key) = self.server_endpoints.get(&message.destination) {
                    scheduler.schedule_now(
                        server_key,
                        TonicServerEvent::RequestReceived {
                            message: message.clone(),
                        },
                    );
                } else {
                    println!(
                        "Warning: No server registered for endpoint {}, dropping request {}",
                        message.destination, message.id
                    );
                }
            }
            MessageType::UnaryResponse => {
                // Route to client
                if let Some(&client_key) = self.client_endpoints.get(&message.destination) {
                    // Deserialize response
                    match utils::deserialize_rpc_response(&message.payload) {
                        Ok(rpc_response) => {
                            let correlation_id = message
                                .correlation_id
                                .unwrap_or_else(|| format!("unknown-{}", message.id));

                            scheduler.schedule_now(
                                client_key,
                                TonicClientEvent::ResponseReceived {
                                    response: rpc_response,
                                    correlation_id,
                                },
                            );
                        }
                        Err(e) => {
                            eprintln!("Failed to deserialize response: {}", e);
                        }
                    }
                } else {
                    println!(
                        "Warning: No client registered for endpoint {}, dropping response {}",
                        message.destination, message.id
                    );
                }
            }
            _ => {
                println!(
                    "Warning: Unsupported message type: {:?}",
                    message.message_type
                );
            }
        }
    }
}

impl Default for TonicTransportRouter {
    fn default() -> Self {
        Self::new()
    }
}

impl Component for TonicTransportRouter {
    type Event = TransportEvent;

    fn process_event(
        &mut self,
        _self_id: Key<Self::Event>,
        event: &Self::Event,
        scheduler: &mut Scheduler,
    ) {
        match event {
            TransportEvent::SendMessage { .. } => {
                // Router doesn't handle send requests, only delivered messages
            }
            TransportEvent::MessageDelivered {
                message,
                destination: _,
            } => {
                self.route_message(message.clone(), scheduler);
            }
            TransportEvent::MessageDropped { message, reason } => {
                println!(
                    "Message {} from {} to {} was dropped: {}",
                    message.id, message.source, message.destination, reason
                );
            }
        }
    }
}
