//! Endpoint registry for service discovery in simulated networks
//!
//! This module provides a registry for managing service endpoints in the simulation,
//! allowing services to be discovered by name and supporting load balancing across
//! multiple instances of the same service.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use des_tokio::sync::notify::Notify;

/// Unique identifier for a service endpoint
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EndpointId {
    id: u64,
}

impl EndpointId {
    /// Create a new endpoint ID from a service name
    pub fn new(service_name: String) -> Self {
        // Use a deterministic hash function instead of DefaultHasher
        // to ensure consistent IDs across Rust versions/platforms
        let hash = Self::djb2_hash(service_name.as_bytes());
        Self { id: hash }
    }

    /// DJB2 hash function for deterministic string hashing
    fn djb2_hash(bytes: &[u8]) -> u64 {
        let mut hash: u64 = 5381;
        for &byte in bytes {
            hash = hash.wrapping_mul(33).wrapping_add(byte as u64);
        }
        hash
    }

    /// Create an endpoint ID from a raw ID (for testing)
    pub fn from_id(id: u64) -> Self {
        Self { id }
    }

    /// Get the raw ID value
    pub fn id(&self) -> u64 {
        self.id
    }
}

impl std::fmt::Display for EndpointId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Endpoint({})", self.id)
    }
}

/// Information about a registered service endpoint
#[derive(Debug, Clone)]
pub struct EndpointInfo {
    /// Unique identifier for this endpoint
    pub id: EndpointId,
    /// Service name (e.g., "user-service")
    pub service_name: String,
    /// Instance identifier (e.g., "user-service-1")
    pub instance_name: String,
    /// Optional metadata for the endpoint
    pub metadata: HashMap<String, String>,
}

impl EndpointInfo {
    /// Create a new endpoint info
    pub fn new(service_name: String, instance_name: String) -> Self {
        let id = EndpointId::new(format!("{service_name}:{instance_name}"));
        Self {
            id,
            service_name,
            instance_name,
            metadata: HashMap::new(),
        }
    }

    /// Add metadata to the endpoint
    pub fn with_metadata(mut self, key: String, value: String) -> Self {
        self.metadata.insert(key, value);
        self
    }
}

/// Trait for endpoint registries
pub trait EndpointRegistry: Send + Sync {
    /// Register a new service endpoint
    fn register_endpoint(&mut self, endpoint: EndpointInfo) -> Result<(), String>;

    /// Unregister a service endpoint
    fn unregister_endpoint(&mut self, endpoint_id: EndpointId) -> Result<(), String>;

    /// Find all endpoints for a service
    fn find_endpoints(&self, service_name: &str) -> Vec<EndpointInfo>;

    /// Find a specific endpoint by ID
    fn find_endpoint(&self, endpoint_id: EndpointId) -> Option<EndpointInfo>;

    /// List all registered services
    fn list_services(&self) -> Vec<String>;

    /// Get endpoint for load balancing (round-robin by default)
    fn get_endpoint_for_service(&mut self, service_name: &str) -> Option<EndpointInfo>;
}

/// Simple in-memory endpoint registry with round-robin load balancing
pub struct SimEndpointRegistry {
    /// Map from endpoint ID to endpoint info
    endpoints: HashMap<EndpointId, EndpointInfo>,
    /// Map from service name to list of endpoint IDs
    services: HashMap<String, Vec<EndpointId>>,
    /// Round-robin counters for load balancing
    round_robin_counters: HashMap<String, usize>,
}

impl SimEndpointRegistry {
    /// Create a new endpoint registry
    pub fn new() -> Self {
        Self {
            endpoints: HashMap::new(),
            services: HashMap::new(),
            round_robin_counters: HashMap::new(),
        }
    }

    /// Create a thread-safe shared registry
    pub fn shared() -> Arc<Mutex<Self>> {
        Arc::new(Mutex::new(Self::new()))
    }
}

impl Default for SimEndpointRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl EndpointRegistry for SimEndpointRegistry {
    fn register_endpoint(&mut self, endpoint: EndpointInfo) -> Result<(), String> {
        let endpoint_id = endpoint.id;
        let service_name = endpoint.service_name.clone();

        // Check if endpoint is already registered
        if self.endpoints.contains_key(&endpoint_id) {
            return Err(format!("Endpoint {endpoint_id} is already registered"));
        }

        // Register the endpoint
        self.endpoints.insert(endpoint_id, endpoint);

        // Add to service list
        self.services
            .entry(service_name.clone())
            .or_default()
            .push(endpoint_id);

        // Initialize round-robin counter if needed
        self.round_robin_counters.entry(service_name).or_insert(0);

        Ok(())
    }

    fn unregister_endpoint(&mut self, endpoint_id: EndpointId) -> Result<(), String> {
        // Find and remove the endpoint
        let endpoint = self
            .endpoints
            .remove(&endpoint_id)
            .ok_or_else(|| format!("Endpoint {endpoint_id} not found"))?;

        // Remove from service list
        if let Some(endpoint_list) = self.services.get_mut(&endpoint.service_name) {
            endpoint_list.retain(|&id| id != endpoint_id);

            // Remove service entry if no endpoints remain
            if endpoint_list.is_empty() {
                self.services.remove(&endpoint.service_name);
                self.round_robin_counters.remove(&endpoint.service_name);
            }
        }

        Ok(())
    }

    fn find_endpoints(&self, service_name: &str) -> Vec<EndpointInfo> {
        self.services
            .get(service_name)
            .map(|endpoint_ids| {
                endpoint_ids
                    .iter()
                    .filter_map(|&id| self.endpoints.get(&id))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    fn find_endpoint(&self, endpoint_id: EndpointId) -> Option<EndpointInfo> {
        self.endpoints.get(&endpoint_id).cloned()
    }

    fn list_services(&self) -> Vec<String> {
        self.services.keys().cloned().collect()
    }

    fn get_endpoint_for_service(&mut self, service_name: &str) -> Option<EndpointInfo> {
        let endpoint_ids = self.services.get(service_name)?;
        if endpoint_ids.is_empty() {
            return None;
        }

        // Round-robin selection
        let counter = self.round_robin_counters.get_mut(service_name)?;
        let selected_id = endpoint_ids[*counter % endpoint_ids.len()];
        *counter = (*counter + 1) % endpoint_ids.len();

        self.endpoints.get(&selected_id).cloned()
    }
}

/// Thread-safe wrapper for endpoint registry
pub struct SharedEndpointRegistry {
    inner: Arc<Mutex<SimEndpointRegistry>>,
    notify: Notify,
}

impl Default for SharedEndpointRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl SharedEndpointRegistry {
    /// Create a new shared endpoint registry
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(SimEndpointRegistry::new())),
            notify: Notify::new(),
        }
    }

    /// Register an endpoint (thread-safe)
    pub fn register_endpoint(&self, endpoint: EndpointInfo) -> Result<(), String> {
        let res = self.inner.lock().unwrap().register_endpoint(endpoint);
        if res.is_ok() {
            self.notify.notify_waiters();
        }
        res
    }

    /// Unregister an endpoint (thread-safe)
    pub fn unregister_endpoint(&self, endpoint_id: EndpointId) -> Result<(), String> {
        let res = self.inner.lock().unwrap().unregister_endpoint(endpoint_id);
        if res.is_ok() {
            self.notify.notify_waiters();
        }
        res
    }

    /// Find endpoints for a service (thread-safe)
    pub fn find_endpoints(&self, service_name: &str) -> Vec<EndpointInfo> {
        self.inner.lock().unwrap().find_endpoints(service_name)
    }

    /// Get endpoint for load balancing (thread-safe)
    pub fn get_endpoint_for_service(&self, service_name: &str) -> Option<EndpointInfo> {
        self.inner
            .lock()
            .unwrap()
            .get_endpoint_for_service(service_name)
    }

    /// Wait until any endpoint registry change occurs.
    pub async fn changed(&self) {
        self.notify.notified().await
    }
}

impl Clone for SharedEndpointRegistry {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
            notify: self.notify.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_endpoint_id_creation() {
        let id1 = EndpointId::new("user-service".to_string());
        let id2 = EndpointId::new("user-service".to_string());
        let id3 = EndpointId::new("order-service".to_string());

        // Same service name should produce same ID
        assert_eq!(id1, id2);
        // Different service names should produce different IDs
        assert_ne!(id1, id3);
    }

    #[test]
    fn test_endpoint_registry_basic() {
        let mut registry = SimEndpointRegistry::new();

        let endpoint1 = EndpointInfo::new("user-service".to_string(), "instance-1".to_string());
        let endpoint2 = EndpointInfo::new("user-service".to_string(), "instance-2".to_string());
        let endpoint3 = EndpointInfo::new("order-service".to_string(), "instance-1".to_string());

        // Register endpoints
        assert!(registry.register_endpoint(endpoint1.clone()).is_ok());
        assert!(registry.register_endpoint(endpoint2.clone()).is_ok());
        assert!(registry.register_endpoint(endpoint3.clone()).is_ok());

        // Find endpoints by service
        let user_endpoints = registry.find_endpoints("user-service");
        assert_eq!(user_endpoints.len(), 2);

        let order_endpoints = registry.find_endpoints("order-service");
        assert_eq!(order_endpoints.len(), 1);

        // List services
        let services = registry.list_services();
        assert_eq!(services.len(), 2);
        assert!(services.contains(&"user-service".to_string()));
        assert!(services.contains(&"order-service".to_string()));
    }

    #[test]
    fn test_round_robin_load_balancing() {
        let mut registry = SimEndpointRegistry::new();

        let endpoint1 = EndpointInfo::new("user-service".to_string(), "instance-1".to_string());
        let endpoint2 = EndpointInfo::new("user-service".to_string(), "instance-2".to_string());

        registry.register_endpoint(endpoint1.clone()).unwrap();
        registry.register_endpoint(endpoint2.clone()).unwrap();

        // Get endpoints in round-robin fashion
        let selected1 = registry.get_endpoint_for_service("user-service").unwrap();
        let selected2 = registry.get_endpoint_for_service("user-service").unwrap();
        let selected3 = registry.get_endpoint_for_service("user-service").unwrap();

        // Should cycle through endpoints
        assert_ne!(selected1.instance_name, selected2.instance_name);
        assert_eq!(selected1.instance_name, selected3.instance_name);
    }

    #[test]
    fn test_endpoint_unregistration() {
        let mut registry = SimEndpointRegistry::new();

        let endpoint = EndpointInfo::new("user-service".to_string(), "instance-1".to_string());
        let endpoint_id = endpoint.id;

        registry.register_endpoint(endpoint).unwrap();

        // Verify endpoint is registered
        assert!(registry.find_endpoint(endpoint_id).is_some());
        assert_eq!(registry.find_endpoints("user-service").len(), 1);

        // Unregister endpoint
        assert!(registry.unregister_endpoint(endpoint_id).is_ok());

        // Verify endpoint is removed
        assert!(registry.find_endpoint(endpoint_id).is_none());
        assert_eq!(registry.find_endpoints("user-service").len(), 0);
        assert_eq!(registry.list_services().len(), 0);
    }

    #[test]
    fn test_shared_endpoint_registry() {
        let registry = SharedEndpointRegistry::new();

        let endpoint = EndpointInfo::new("user-service".to_string(), "instance-1".to_string());

        // Test thread-safe operations
        assert!(registry.register_endpoint(endpoint.clone()).is_ok());

        let endpoints = registry.find_endpoints("user-service");
        assert_eq!(endpoints.len(), 1);
        assert_eq!(endpoints[0].service_name, "user-service");

        let selected = registry.get_endpoint_for_service("user-service");
        assert!(selected.is_some());
        assert_eq!(selected.unwrap().service_name, "user-service");
    }
}
