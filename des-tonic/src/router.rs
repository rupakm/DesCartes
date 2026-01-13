use bytes::Bytes;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tonic::{Request, Response, Status};

pub type UnaryFuture = Pin<Box<dyn Future<Output = Result<Response<Bytes>, Status>> + Send>>;
pub type UnaryHandler = Arc<dyn Fn(Request<Bytes>) -> UnaryFuture + Send + Sync + 'static>;

/// Router maps method paths ("/pkg.Service/Method") to unary handlers.
#[derive(Clone, Default)]
pub struct Router {
    unary: HashMap<String, UnaryHandler>,
}

impl Router {
    #[must_use]
    pub fn new() -> Self {
        Self {
            unary: HashMap::new(),
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

    pub(crate) fn unary(&self, path: &str) -> Option<&UnaryHandler> {
        self.unary.get(path)
    }

    pub fn list_unary(&self) -> Vec<String> {
        let mut v: Vec<_> = self.unary.keys().cloned().collect();
        v.sort();
        v
    }
}
