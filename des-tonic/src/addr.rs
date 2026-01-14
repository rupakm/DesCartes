use std::net::SocketAddr;

use tonic::Status;

pub(crate) fn parse_socket_addr(input: &str) -> Result<SocketAddr, Status> {
    // Accept plain "127.0.0.1:50051" and URI-like "http://127.0.0.1:50051".
    let trimmed = input.trim();

    let without_scheme = trimmed
        .strip_prefix("http://")
        .or_else(|| trimmed.strip_prefix("https://"))
        .unwrap_or(trimmed);

    let host_port = without_scheme
        .split_once('/')
        .map(|(hp, _)| hp)
        .unwrap_or(without_scheme);

    host_port
        .parse::<SocketAddr>()
        .map_err(|e| Status::invalid_argument(format!("invalid socket address '{input}': {e}")))
}
