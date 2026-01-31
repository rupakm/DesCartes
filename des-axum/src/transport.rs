use crate::addr::parse_socket_addr;
use crate::client::{Client, ClientBuilder, Error, InstalledClient};
use crate::server::{InstalledServer, ServerBuilder};
use des_components::transport::{
    EndpointId, NetworkModel, SimTransport, SimpleNetworkModel, TransportEvent,
};
use des_core::{Key, SchedulerHandle, Simulation};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Clone)]
pub struct Transport {
    pub transport_key: Key<TransportEvent>,
    pub endpoint_registry: des_components::transport::SharedEndpointRegistry,
    pub scheduler: SchedulerHandle,

    next_client_id: Arc<Mutex<u64>>,
}

impl Transport {
    /// Install a `SimTransport` component and return a handle to it.
    pub fn install(sim: &mut Simulation, network_model: Box<dyn NetworkModel>) -> Self {
        let transport = SimTransport::new(network_model);
        let endpoint_registry = transport.endpoint_registry().clone();
        let transport_key = sim.add_component(transport);

        Self {
            transport_key,
            endpoint_registry,
            scheduler: sim.scheduler_handle(),
            next_client_id: Arc::new(Mutex::new(0)),
        }
    }

    /// Install a deterministic transport with zero latency and no packet loss.
    pub fn install_default(sim: &mut Simulation) -> Self {
        // Deterministic by default: base latency 0, no packet loss.
        let seed = sim.config().seed ^ 0x0D15_EAA7_u64;
        let model = SimpleNetworkModel::with_seed(Duration::ZERO, 0.0, seed);
        Self::install(sim, Box::new(model))
    }

    fn next_client_name(&self, service_name: &str) -> String {
        let mut c = self.next_client_id.lock().unwrap();
        *c += 1;
        format!("{service_name}:{}", *c)
    }

    fn client_builder(&self, service_name: String) -> ClientBuilder {
        ClientBuilder::new(
            service_name,
            self.transport_key,
            self.endpoint_registry.clone(),
            self.scheduler.clone(),
        )
    }

    /// Serve an axum router identified by `(service_name, instance_name)`.
    pub fn serve_named(
        &self,
        sim: &mut Simulation,
        service_name: impl Into<String>,
        instance_name: impl Into<String>,
        app: axum::Router,
    ) -> Result<InstalledServer, Error> {
        ServerBuilder::new(
            service_name.into(),
            instance_name.into(),
            self.transport_key,
            self.endpoint_registry.clone(),
            self.scheduler.clone(),
        )
        .app(app)
        .install(sim)
    }

    /// Convenience wrapper: `instance_name = addr.to_string()`.
    pub fn serve_socket_addr(
        &self,
        sim: &mut Simulation,
        service_name: impl Into<String>,
        addr: SocketAddr,
        app: axum::Router,
    ) -> Result<InstalledServer, Error> {
        self.serve_named(sim, service_name, addr.to_string(), app)
    }

    /// Convenience wrapper: accept "127.0.0.1:3000" or "http://127.0.0.1:3000".
    pub fn serve(
        &self,
        sim: &mut Simulation,
        service_name: impl Into<String>,
        addr: impl AsRef<str>,
        app: axum::Router,
    ) -> Result<InstalledServer, Error> {
        let addr = parse_socket_addr(addr.as_ref())?;
        self.serve_socket_addr(sim, service_name, addr, app)
    }

    /// Create and install a client endpoint that load-balances across instances.
    pub fn connect(
        &self,
        sim: &mut Simulation,
        service_name: impl Into<String>,
    ) -> Result<Client, Error> {
        let service_name = service_name.into();
        let client_name = self.next_client_name(&service_name);
        let installed = self
            .client_builder(service_name)
            .client_name(client_name)
            .install(sim)?;
        Ok(installed.client)
    }

    /// Create and install a client endpoint pinned to a specific instance (`SocketAddr`).
    pub fn connect_socket_addr(
        &self,
        sim: &mut Simulation,
        service_name: impl Into<String>,
        addr: SocketAddr,
    ) -> Result<Client, Error> {
        let service_name = service_name.into();
        let instance_name = addr.to_string();
        let target_endpoint = EndpointId::new(format!("{service_name}:{instance_name}"));

        let found = self
            .endpoint_registry
            .find_endpoints(&service_name)
            .iter()
            .any(|e| e.id == target_endpoint);
        if !found {
            return Err(Error::NotFound(format!(
                "service not found at {addr} ({service_name})"
            )));
        }

        let client_name = self.next_client_name(&service_name);
        let installed = self
            .client_builder(service_name)
            .client_name(client_name)
            .target_endpoint(target_endpoint)
            .install(sim)?;
        Ok(installed.client)
    }

    /// Convenience wrapper: accept "127.0.0.1:3000" or "http://127.0.0.1:3000".
    pub fn connect_addr(
        &self,
        sim: &mut Simulation,
        service_name: impl Into<String>,
        addr: impl AsRef<str>,
    ) -> Result<Client, Error> {
        let addr = parse_socket_addr(addr.as_ref())?;
        self.connect_socket_addr(sim, service_name, addr)
    }

    /// Install a client endpoint component and return the installed artifacts.
    pub fn install_client(
        &self,
        sim: &mut Simulation,
        service_name: impl Into<String>,
    ) -> Result<InstalledClient, Error> {
        let service_name = service_name.into();
        let client_name = self.next_client_name(&service_name);
        self.client_builder(service_name)
            .client_name(client_name)
            .install(sim)
    }
}
