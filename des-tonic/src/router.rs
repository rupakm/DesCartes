use crate::stream::DesStreaming;
use bytes::Bytes;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tonic::{Request, Response, Status};

pub type UnaryFuture = Pin<Box<dyn Future<Output = Result<Response<Bytes>, Status>> + Send>>;
pub type UnaryHandler = Arc<dyn Fn(Request<Bytes>) -> UnaryFuture + Send + Sync + 'static>;

pub type ServerStreamingFuture =
    Pin<Box<dyn Future<Output = Result<Response<DesStreaming<Bytes>>, Status>> + Send>>;
pub type ServerStreamingHandler =
    Arc<dyn Fn(Request<Bytes>) -> ServerStreamingFuture + Send + Sync + 'static>;

pub type ClientStreamingFuture =
    Pin<Box<dyn Future<Output = Result<Response<Bytes>, Status>> + Send>>;
pub type ClientStreamingHandler =
    Arc<dyn Fn(Request<DesStreaming<Bytes>>) -> ClientStreamingFuture + Send + Sync + 'static>;

pub type BidiStreamingFuture =
    Pin<Box<dyn Future<Output = Result<Response<DesStreaming<Bytes>>, Status>> + Send>>;
pub type BidiStreamingHandler =
    Arc<dyn Fn(Request<DesStreaming<Bytes>>) -> BidiStreamingFuture + Send + Sync + 'static>;

/// Router maps method paths ("/pkg.Service/Method") to handler implementations.
#[derive(Clone, Default)]
pub struct Router {
    unary: HashMap<String, UnaryHandler>,
    server_streaming: HashMap<String, ServerStreamingHandler>,
    client_streaming: HashMap<String, ClientStreamingHandler>,
    bidi_streaming: HashMap<String, BidiStreamingHandler>,
}

impl Router {
    #[must_use]
    pub fn new() -> Self {
        Self {
            unary: HashMap::new(),
            server_streaming: HashMap::new(),
            client_streaming: HashMap::new(),
            bidi_streaming: HashMap::new(),
        }
    }

    /// Register a unary handler for a fully-qualified method path.
    ///
    /// `path` should look like "/pkg.Service/Method".
    pub fn add_unary<F, Fut>(&mut self, path: impl Into<String>, handler: F) -> &mut Self
    where
        F: Fn(Request<Bytes>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Response<Bytes>, Status>> + Send + 'static,
    {
        let handler: UnaryHandler = Arc::new(move |req| Box::pin(handler(req)));
        self.unary.insert(path.into(), handler);
        self
    }

    /// Register a server-streaming handler.
    ///
    /// Signature: `Request<Bytes> -> Response<DesStreaming<Bytes>>`.
    pub fn add_server_streaming<F, Fut>(&mut self, path: impl Into<String>, handler: F) -> &mut Self
    where
        F: Fn(Request<Bytes>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Response<DesStreaming<Bytes>>, Status>> + Send + 'static,
    {
        let handler: ServerStreamingHandler = Arc::new(move |req| Box::pin(handler(req)));
        self.server_streaming.insert(path.into(), handler);
        self
    }

    /// Register a client-streaming handler.
    ///
    /// Signature: `Request<DesStreaming<Bytes>> -> Response<Bytes>`.
    pub fn add_client_streaming<F, Fut>(&mut self, path: impl Into<String>, handler: F) -> &mut Self
    where
        F: Fn(Request<DesStreaming<Bytes>>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Response<Bytes>, Status>> + Send + 'static,
    {
        let handler: ClientStreamingHandler = Arc::new(move |req| Box::pin(handler(req)));
        self.client_streaming.insert(path.into(), handler);
        self
    }

    /// Register a bidirectional-streaming handler.
    ///
    /// Signature: `Request<DesStreaming<Bytes>> -> Response<DesStreaming<Bytes>>`.
    pub fn add_bidi_streaming<F, Fut>(&mut self, path: impl Into<String>, handler: F) -> &mut Self
    where
        F: Fn(Request<DesStreaming<Bytes>>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Response<DesStreaming<Bytes>>, Status>> + Send + 'static,
    {
        let handler: BidiStreamingHandler = Arc::new(move |req| Box::pin(handler(req)));
        self.bidi_streaming.insert(path.into(), handler);
        self
    }

    pub(crate) fn unary(&self, path: &str) -> Option<&UnaryHandler> {
        self.unary.get(path)
    }

    #[allow(dead_code)]
    pub(crate) fn server_streaming(&self, path: &str) -> Option<&ServerStreamingHandler> {
        self.server_streaming.get(path)
    }

    #[allow(dead_code)]
    pub(crate) fn client_streaming(&self, path: &str) -> Option<&ClientStreamingHandler> {
        self.client_streaming.get(path)
    }

    #[allow(dead_code)]
    pub(crate) fn bidi_streaming(&self, path: &str) -> Option<&BidiStreamingHandler> {
        self.bidi_streaming.get(path)
    }

    pub fn list_unary(&self) -> Vec<String> {
        let mut v: Vec<_> = self.unary.keys().cloned().collect();
        v.sort();
        v
    }

    pub fn list_server_streaming(&self) -> Vec<String> {
        let mut v: Vec<_> = self.server_streaming.keys().cloned().collect();
        v.sort();
        v
    }

    pub fn list_client_streaming(&self) -> Vec<String> {
        let mut v: Vec<_> = self.client_streaming.keys().cloned().collect();
        v.sort();
        v
    }

    pub fn list_bidi_streaming(&self) -> Vec<String> {
        let mut v: Vec<_> = self.bidi_streaming.keys().cloned().collect();
        v.sort();
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn router_lists_methods_by_kind() {
        let mut router = Router::new();
        router
            .add_unary(
                "/svc.U/A",
                |_req| async move { Ok(Response::new(Bytes::new())) },
            )
            .add_server_streaming("/svc.S/S", |_req| async move {
                let (_tx, rx) = des_tokio::sync::mpsc::channel(1);
                Ok(Response::new(DesStreaming::new(rx)))
            })
            .add_client_streaming(
                "/svc.C/C",
                |_req| async move { Ok(Response::new(Bytes::new())) },
            )
            .add_bidi_streaming("/svc.B/B", |_req| async move {
                let (_tx, rx) = des_tokio::sync::mpsc::channel(1);
                Ok(Response::new(DesStreaming::new(rx)))
            });

        assert_eq!(router.list_unary(), vec!["/svc.U/A".to_string()]);
        assert_eq!(router.list_server_streaming(), vec!["/svc.S/S".to_string()]);
        assert_eq!(router.list_client_streaming(), vec!["/svc.C/C".to_string()]);
        assert_eq!(router.list_bidi_streaming(), vec!["/svc.B/B".to_string()]);
    }
}
