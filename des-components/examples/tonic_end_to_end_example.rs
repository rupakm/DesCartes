//! Real DES simulation example demonstrating tonic-style RPC with periodic client requests
//!
//! This example shows how to:
//! 1. Create DES components that integrate tonic clients and servers with transport
//! 2. Set up clients that send periodic requests at constant rates
//! 3. Route messages through the transport layer with realistic network simulation
//! 4. Observe actual discrete event simulation behavior

use des_components::{
    DesTonicClient, DesTonicClientBuilder, DesTonicServer, DesTonicServerBuilder, JsonCodec,
    LatencyConfig, LatencyJitterModel, RpcCodec, RpcRequest, RpcResponse, RpcService,
    RpcStatusCode, SharedEndpointRegistry, SimTransport, TonicClientEvent, TonicServerEvent,
    TonicTransportRouter, TransportEvent,
};
use des_core::{Execute, Executor, SimTime, Simulation};
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// User service request message
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
struct GetUserRequest {
    user_id: u32,
}

/// User service response message
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
struct GetUserResponse {
    user_id: u32,
    name: String,
    email: String,
}

/// User service implementation
struct UserService {
    users: std::collections::HashMap<u32, (String, String)>,
}

impl UserService {
    fn new() -> Self {
        let mut users = std::collections::HashMap::new();
        users.insert(1, ("Alice".to_string(), "alice@example.com".to_string()));
        users.insert(2, ("Bob".to_string(), "bob@example.com".to_string()));
        users.insert(
            3,
            ("Charlie".to_string(), "charlie@example.com".to_string()),
        );

        Self { users }
    }
}

impl RpcService for UserService {
    fn handle_request(
        &mut self,
        request: RpcRequest,
        _scheduler_handle: &des_core::SchedulerHandle,
    ) -> Result<RpcResponse, des_components::TonicError> {
        match request.method.method.as_str() {
            "GetUser" => {
                // Decode request
                let codec = JsonCodec::<GetUserRequest>::new();
                let get_user_req = codec.decode(&request.payload)?;

                // Look up user (processing delay is handled by DesTonicServer)
                if let Some((name, email)) = self.users.get(&get_user_req.user_id) {
                    let response = GetUserResponse {
                        user_id: get_user_req.user_id,
                        name: name.clone(),
                        email: email.clone(),
                    };

                    // Encode response
                    let response_codec = JsonCodec::<GetUserResponse>::new();
                    let response_payload = response_codec.encode(&response)?;

                    Ok(RpcResponse::success(response_payload))
                } else {
                    Ok(RpcResponse::error(
                        RpcStatusCode::NotFound,
                        format!("User {} not found", get_user_req.user_id),
                    ))
                }
            }
            _ => Err(des_components::TonicError::MethodNotFound {
                service: request.method.service,
                method: request.method.method,
            }),
        }
    }

    fn service_name(&self) -> &str {
        "user.UserService"
    }

    fn supported_methods(&self) -> Vec<String> {
        vec!["GetUser".to_string()]
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("🚀 Real Tonic DES Simulation Example");
    println!("====================================\n");

    let mut sim = Simulation::default();

    // 1. Create network with low latency for testing
    println!("📡 Setting up simulated network...");
    let network_config = LatencyConfig::new(Duration::from_millis(10))
        .with_jitter(0.1) // 10% jitter
        .with_packet_loss(0.0); // No packet loss for testing

    let network_model = Box::new(LatencyJitterModel::with_seed(network_config, 42));
    let transport = SimTransport::new(network_model);
    let transport_key = sim.add_component(transport);

    // 2. Create shared endpoint registry
    let endpoint_registry = SharedEndpointRegistry::new();

    // 3. Create transport router for routing messages between clients and servers
    println!("🔀 Setting up transport router...");
    let router = TonicTransportRouter::new();
    let router_key = sim.add_component(router);

    // 4. Create and start the user service server
    println!("🖥️  Starting UserService server...");
    let user_service = UserService::new();
    let server = DesTonicServerBuilder::new()
        .name("UserServer".to_string())
        .service_name("user.UserService".to_string())
        .instance_name("server-1".to_string())
        .transport_key(transport_key) // Connect to transport, not router
        .endpoint_registry(endpoint_registry.clone())
        .add_service(user_service)
        .timeout(Duration::from_secs(5))
        .build()?;

    server.start()?;
    let server_key = sim.add_component(server);
    println!("   ✅ Server registered and listening");

    // 5. Create multiple clients that send periodic requests
    println!("👥 Creating periodic RPC clients...");

    // Client 1: Requests user 1 every 2 seconds
    let client1 = DesTonicClientBuilder::<GetUserRequest>::new()
        .name("Client1".to_string())
        .service_name("user.UserService".to_string())
        .method_name("GetUser".to_string())
        .transport_key(transport_key) // Connect to transport, not router
        .endpoint_registry(endpoint_registry.clone())
        .codec(Box::new(JsonCodec::new()))
        .request_generator(Box::new(|_count| GetUserRequest { user_id: 1 }))
        .interval(Duration::from_secs(2))
        .timeout(Duration::from_secs(3))
        .build()?;

    let client1_key = sim.add_component(client1);

    // Client 2: Requests user 2 every 3 seconds
    let client2 = DesTonicClientBuilder::<GetUserRequest>::new()
        .name("Client2".to_string())
        .service_name("user.UserService".to_string())
        .method_name("GetUser".to_string())
        .transport_key(transport_key) // Connect to transport, not router
        .endpoint_registry(endpoint_registry.clone())
        .codec(Box::new(JsonCodec::new()))
        .request_generator(Box::new(|_count| GetUserRequest { user_id: 2 }))
        .interval(Duration::from_secs(3))
        .timeout(Duration::from_secs(3))
        .build()?;

    let client2_key = sim.add_component(client2);

    // Client 3: Requests non-existent user every 1.5 seconds (to test error handling)
    let client3 = DesTonicClientBuilder::<GetUserRequest>::new()
        .name("Client3".to_string())
        .service_name("user.UserService".to_string())
        .method_name("GetUser".to_string())
        .transport_key(transport_key) // Connect to transport, not router
        .endpoint_registry(endpoint_registry.clone())
        .codec(Box::new(JsonCodec::new()))
        .request_generator(Box::new(|_count| GetUserRequest { user_id: 999 }))
        .interval(Duration::from_millis(1500))
        .timeout(Duration::from_secs(3))
        .build()?;

    let client3_key = sim.add_component(client3);

    println!("   ✅ Clients created and configured");

    // 6. Configure transport to forward messages to router
    println!("🔗 Configuring transport message routing...");

    // Get endpoint IDs from components (need to do this separately to avoid borrow conflicts)
    let server_endpoint_id = {
        let server_ref = sim
            .get_component_mut::<TonicServerEvent, DesTonicServer>(server_key)
            .unwrap();
        server_ref.endpoint_id()
    };

    let client1_endpoint_id = {
        let client1_ref = sim
            .get_component_mut::<TonicClientEvent, DesTonicClient<GetUserRequest>>(client1_key)
            .unwrap();
        client1_ref.endpoint_id
    };

    let client2_endpoint_id = {
        let client2_ref = sim
            .get_component_mut::<TonicClientEvent, DesTonicClient<GetUserRequest>>(client2_key)
            .unwrap();
        client2_ref.endpoint_id
    };

    let client3_endpoint_id = {
        let client3_ref = sim
            .get_component_mut::<TonicClientEvent, DesTonicClient<GetUserRequest>>(client3_key)
            .unwrap();
        client3_ref.endpoint_id
    };

    // Register router with transport for all endpoints
    let transport_ref = sim
        .get_component_mut::<TransportEvent, SimTransport>(transport_key)
        .unwrap();
    transport_ref.register_handler(server_endpoint_id, router_key);
    transport_ref.register_handler(client1_endpoint_id, router_key);
    transport_ref.register_handler(client2_endpoint_id, router_key);
    transport_ref.register_handler(client3_endpoint_id, router_key);

    // Register clients and server with the router
    let router_ref = sim
        .get_component_mut::<TransportEvent, TonicTransportRouter>(router_key)
        .unwrap();
    router_ref.register_server(server_endpoint_id, server_key);
    router_ref.register_client(client1_endpoint_id, client1_key);
    router_ref.register_client(client2_endpoint_id, client2_key);
    router_ref.register_client(client3_endpoint_id, client3_key);

    println!("   ✅ Transport routing configured and components registered");

    // 7. Start periodic requests by scheduling initial events
    println!("\n🔄 Starting periodic client requests...");
    sim.schedule(
        SimTime::zero(),
        client1_key,
        TonicClientEvent::SendPeriodicRequest,
    );
    sim.schedule(
        SimTime::zero(),
        client2_key,
        TonicClientEvent::SendPeriodicRequest,
    );
    sim.schedule(
        SimTime::zero(),
        client3_key,
        TonicClientEvent::SendPeriodicRequest,
    );

    // 8. Run simulation for 20 seconds to see network latency effects
    println!("\n⏱️  Running simulation for 20 seconds...");
    Executor::timed(SimTime::from_duration(Duration::from_secs(60))).execute(&mut sim);

    // 9. Display results
    println!("\n📊 Simulation Results:");
    println!("=====================");

    // Get transport statistics
    let transport_ref = sim
        .get_component_mut::<TransportEvent, SimTransport>(transport_key)
        .unwrap();
    let transport_stats = transport_ref.stats();

    println!("📈 Network Statistics:");
    println!("   Messages sent: {}", transport_stats.messages_sent);
    println!(
        "   Messages delivered: {}",
        transport_stats.messages_delivered
    );
    println!("   Messages dropped: {}", transport_stats.messages_dropped);
    println!("   Bytes sent: {}", transport_stats.bytes_sent);
    println!("   Bytes delivered: {}", transport_stats.bytes_delivered);

    if transport_stats.messages_sent > 0 {
        let delivery_rate = (transport_stats.messages_delivered as f64
            / transport_stats.messages_sent as f64)
            * 100.0;
        println!("   Delivery rate: {:.1}%", delivery_rate);
    }

    // Get client statistics
    println!("\n📊 Client Statistics:");
    let client1_ref = sim
        .get_component_mut::<TonicClientEvent, DesTonicClient<GetUserRequest>>(client1_key)
        .unwrap();
    let client1_stats = client1_ref.stats();
    println!(
        "   Client1: {} sent, {} received, {} timeouts, {} successful, {} failed",
        client1_stats.requests_sent,
        client1_stats.responses_received,
        client1_stats.requests_timed_out,
        client1_stats.successful_responses,
        client1_stats.failed_responses
    );

    let client2_ref = sim
        .get_component_mut::<TonicClientEvent, DesTonicClient<GetUserRequest>>(client2_key)
        .unwrap();
    let client2_stats = client2_ref.stats();
    println!(
        "   Client2: {} sent, {} received, {} timeouts, {} successful, {} failed",
        client2_stats.requests_sent,
        client2_stats.responses_received,
        client2_stats.requests_timed_out,
        client2_stats.successful_responses,
        client2_stats.failed_responses
    );

    let client3_ref = sim
        .get_component_mut::<TonicClientEvent, DesTonicClient<GetUserRequest>>(client3_key)
        .unwrap();
    let client3_stats = client3_ref.stats();
    println!(
        "   Client3: {} sent, {} received, {} timeouts, {} successful, {} failed",
        client3_stats.requests_sent,
        client3_stats.responses_received,
        client3_stats.requests_timed_out,
        client3_stats.successful_responses,
        client3_stats.failed_responses
    );

    // Get server statistics
    println!("\n📊 Server Statistics:");
    let server_ref = sim
        .get_component_mut::<TonicServerEvent, DesTonicServer>(server_key)
        .unwrap();
    let server_stats = server_ref.stats();
    println!("   Requests received: {}", server_stats.requests_received);
    println!("   Requests processed: {}", server_stats.requests_processed);
    println!("   Requests failed: {}", server_stats.requests_failed);
    println!("   Responses sent: {}", server_stats.responses_sent);

    println!("\n🎯 Key Features Demonstrated:");
    println!("   ✅ Real DES integration with tonic-style RPC");
    println!("   ✅ Periodic client requests at different rates");
    println!("   ✅ Message routing through transport layer");
    println!("   ✅ Deterministic network simulation with latency/jitter");
    println!("   ✅ Error handling for non-existent resources");
    println!("   ✅ Request/response correlation and timeout handling");
    println!("   ✅ Comprehensive statistics collection");

    Ok(())
}
