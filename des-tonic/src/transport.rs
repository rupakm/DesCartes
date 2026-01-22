use std::sync::{Arc, Mutex};
use std::time::Duration;

use des_components::transport::{
    SharedEndpointRegistry, SimTransport, SimpleNetworkModel, TransportEvent,
};

use crate::network::NetworkModel;
use des_core::{Key, SchedulerHandle, Simulation};

use tonic::Status;

use std::net::SocketAddr;

use crate::addr::parse_socket_addr;
use crate::{Channel, Router, ServerBuilder};

#[derive(Clone)]
pub struct Transport {
    pub transport_key: Key<TransportEvent>,
    pub endpoint_registry: SharedEndpointRegistry,
    pub scheduler: SchedulerHandle,

    next_client_id: Arc<Mutex<u64>>,
}

impl Transport {
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

    pub fn install_default(sim: &mut Simulation) -> Self {
        // Deterministic by default: base latency 0, no packet loss.
        let seed = sim.config().seed ^ 0xD15E_A440_5EED_0001u64;
        let model = SimpleNetworkModel::with_seed(Duration::ZERO, 0.0, seed);
        Self::install(sim, Box::new(model))
    }

    fn next_client_endpoint_name(&self, service_name: &str) -> String {
        let mut c = self.next_client_id.lock().unwrap();
        *c += 1;
        format!("{service_name}:{}", *c)
    }

    fn serve_addr(
        &self,
        sim: &mut Simulation,
        service_name: String,
        addr: SocketAddr,
        router: Router,
    ) -> Result<crate::InstalledServer, Status> {
        let instance_name = addr.to_string();

        ServerBuilder::new(
            service_name,
            instance_name,
            self.transport_key,
            self.endpoint_registry.clone(),
            self.scheduler.clone(),
        )
        .add_router(router)
        .install(sim)
        .map_err(|status| {
            if status.code() == tonic::Code::Internal
                && status.message().contains("already registered")
            {
                Status::already_exists(status.message().to_string())
            } else {
                status
            }
        })
    }

    /// Install a server endpoint bound to `addr`.
    ///
    /// If another server is already bound at that address for the same service, this
    /// returns `Status::already_exists`.
    pub fn serve_socket_addr(
        &self,
        sim: &mut Simulation,
        service_name: impl Into<String>,
        addr: SocketAddr,
        router: Router,
    ) -> Result<crate::InstalledServer, Status> {
        self.serve_addr(sim, service_name.into(), addr, router)
    }

    /// Install a server endpoint bound to an address string.
    ///
    /// `addr` may be a plain `SocketAddr` string ("127.0.0.1:50051") or a URI-like
    /// string ("http://127.0.0.1:50051").
    pub fn serve(
        &self,
        sim: &mut Simulation,
        service_name: impl Into<String>,
        addr: impl AsRef<str>,
        router: Router,
    ) -> Result<crate::InstalledServer, Status> {
        let service_name = service_name.into();
        let addr = parse_socket_addr(addr.as_ref())?;
        self.serve_addr(sim, service_name, addr, router)
    }

    /// Install a client endpoint and return a channel connected to `addr`.
    pub fn connect_socket_addr(
        &self,
        sim: &mut Simulation,
        service_name: impl Into<String>,
        addr: SocketAddr,
    ) -> Result<Channel, Status> {
        let service_name = service_name.into();
        let client_name = self.next_client_endpoint_name(&service_name);
        Channel::connect_addr(sim, self, service_name, addr, client_name)
    }

    /// Install a client endpoint and return a channel connected to `addr`.
    ///
    /// `addr` may be a plain `SocketAddr` string ("127.0.0.1:50051") or a URI-like
    /// string ("http://127.0.0.1:50051").
    pub fn connect(
        &self,
        sim: &mut Simulation,
        service_name: impl Into<String>,
        addr: impl AsRef<str>,
    ) -> Result<Channel, Status> {
        let addr = parse_socket_addr(addr.as_ref())?;
        self.connect_socket_addr(sim, service_name, addr)
    }
}
