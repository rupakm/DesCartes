//! Service discovery helpers for DES simulations.
//!
//! In real Tower-based clients (e.g. tonic's `Channel::balance_channel`) a request may remain
//! pending while the balancer waits for endpoints to be discovered.
//!
//! In DES, we usually have an explicit endpoint registry. The helpers in this module bridge the
//! gap by allowing services/clients to wait for endpoint availability in *simulation time*.

use des_components::transport::{EndpointInfo, SharedEndpointRegistry};
use std::time::Duration;

/// Wait for an endpoint to become available for `service_name`.
///
/// - Returns immediately if an endpoint exists.
/// - Otherwise, waits for registry change notifications.
/// - If `timeout` is set, returns `None` after that duration (simulation time).
pub async fn wait_for_endpoint(
    registry: SharedEndpointRegistry,
    service_name: String,
    timeout: Option<Duration>,
) -> Option<EndpointInfo> {
    let deadline = timeout.map(|d| des_tokio::time::Instant::now() + d);

    loop {
        if let Some(ep) = registry.get_endpoint_for_service(&service_name) {
            return Some(ep);
        }


        match deadline {
            None => {
                registry.changed().await;
            }
            Some(d) => {
                let now = des_tokio::time::Instant::now();
                if now >= d {
                    return None;
                }

                let remaining = d.saturating_duration_since(now);
                if des_tokio::time::timeout(remaining, registry.changed()).await.is_err() {
                    return None;
                }
            }
        }
    }
}
