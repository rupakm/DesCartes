use crate::stream::DesStreaming;
use crate::stream::{self};
use bytes::Bytes;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tonic::{Request, Response, Status};

use prost::Message;

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

    pub fn add_unary_prost<S, Req, Resp, Fut>(
        &mut self,
        path: &'static str,
        svc: Arc<S>,
        f: fn(Arc<S>, Request<Req>) -> Fut,
    ) -> &mut Self
    where
        S: Send + Sync + 'static,
        Req: Message + Default + 'static,
        Resp: Message + 'static,
        Fut: Future<Output = Result<Response<Resp>, Status>> + Send + 'static,
    {
        self.add_unary(path, move |req_bytes: Request<Bytes>| {
            let svc = Arc::clone(&svc);
            async move {
                let (metadata, extensions, payload) = req_bytes.into_parts();
                let decoded = Req::decode(payload)
                    .map_err(|e| Status::internal(format!("prost decode error: {e}")))?;
                let req = Request::from_parts(metadata, extensions, decoded);

                let resp = f(svc, req).await?;
                let (metadata, msg, extensions) = resp.into_parts();
                Ok(Response::from_parts(
                    metadata,
                    Bytes::from(msg.encode_to_vec()),
                    extensions,
                ))
            }
        })
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

    pub fn add_server_streaming_prost<S, Req, Resp, Fut>(
        &mut self,
        path: &'static str,
        svc: Arc<S>,
        f: fn(Arc<S>, Request<Req>) -> Fut,
    ) -> &mut Self
    where
        S: Send + Sync + 'static,
        Req: Message + Default + 'static,
        Resp: Message + 'static,
        Fut: Future<Output = Result<Response<DesStreaming<Resp>>, Status>> + Send + 'static,
    {
        self.add_server_streaming(path, move |req_bytes: Request<Bytes>| {
            let svc = Arc::clone(&svc);
            async move {
                let (metadata, extensions, payload) = req_bytes.into_parts();
                let decoded = Req::decode(payload)
                    .map_err(|e| Status::internal(format!("prost decode error: {e}")))?;
                let req = Request::from_parts(metadata, extensions, decoded);

                let resp = f(svc, req).await?;
                let (metadata, mut typed_stream, extensions) = resp.into_parts();

                let (tx_bytes, bytes_stream) = stream::channel::<Bytes>(16);
                descartes_tokio::task::spawn_local(async move {
                    while let Some(item) = typed_stream.next().await {
                        match item {
                            Ok(msg) => {
                                if tx_bytes
                                    .send(Bytes::from(msg.encode_to_vec()))
                                    .await
                                    .is_err()
                                {
                                    break;
                                }
                            }
                            Err(status) => {
                                let _ = tx_bytes.send_err(status).await;
                                break;
                            }
                        }
                    }
                    tx_bytes.close();
                });

                Ok(Response::from_parts(metadata, bytes_stream, extensions))
            }
        })
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

    pub fn add_client_streaming_prost<S, Req, Resp, Fut>(
        &mut self,
        path: &'static str,
        svc: Arc<S>,
        f: fn(Arc<S>, Request<DesStreaming<Req>>) -> Fut,
    ) -> &mut Self
    where
        S: Send + Sync + 'static,
        Req: Message + Default + 'static,
        Resp: Message + 'static,
        Fut: Future<Output = Result<Response<Resp>, Status>> + Send + 'static,
    {
        self.add_client_streaming(path, move |req_bytes: Request<DesStreaming<Bytes>>| {
            let svc = Arc::clone(&svc);
            async move {
                let (metadata, extensions, mut bytes_stream) = req_bytes.into_parts();

                let (tx_typed, typed_stream) = stream::channel::<Req>(16);
                descartes_tokio::task::spawn_local(async move {
                    while let Some(item) = bytes_stream.next().await {
                        match item {
                            Ok(bytes) => match Req::decode(bytes) {
                                Ok(msg) => {
                                    if tx_typed.send(msg).await.is_err() {
                                        break;
                                    }
                                }
                                Err(e) => {
                                    let _ = tx_typed
                                        .send_err(Status::internal(format!(
                                            "prost decode error: {e}"
                                        )))
                                        .await;
                                    break;
                                }
                            },
                            Err(status) => {
                                let _ = tx_typed.send_err(status).await;
                                break;
                            }
                        }
                    }
                    tx_typed.close();
                });

                let req = Request::from_parts(metadata, extensions, typed_stream);
                let resp = f(svc, req).await?;
                let (metadata, msg, extensions) = resp.into_parts();
                Ok(Response::from_parts(
                    metadata,
                    Bytes::from(msg.encode_to_vec()),
                    extensions,
                ))
            }
        })
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

    pub fn add_bidi_streaming_prost<S, Req, Resp, Fut>(
        &mut self,
        path: &'static str,
        svc: Arc<S>,
        f: fn(Arc<S>, Request<DesStreaming<Req>>) -> Fut,
    ) -> &mut Self
    where
        S: Send + Sync + 'static,
        Req: Message + Default + 'static,
        Resp: Message + 'static,
        Fut: Future<Output = Result<Response<DesStreaming<Resp>>, Status>> + Send + 'static,
    {
        self.add_bidi_streaming(path, move |req_bytes: Request<DesStreaming<Bytes>>| {
            let svc = Arc::clone(&svc);
            async move {
                let (metadata, extensions, mut bytes_stream) = req_bytes.into_parts();

                let (tx_typed, typed_stream) = stream::channel::<Req>(16);
                descartes_tokio::task::spawn_local(async move {
                    while let Some(item) = bytes_stream.next().await {
                        match item {
                            Ok(bytes) => match Req::decode(bytes) {
                                Ok(msg) => {
                                    if tx_typed.send(msg).await.is_err() {
                                        break;
                                    }
                                }
                                Err(e) => {
                                    let _ = tx_typed
                                        .send_err(Status::internal(format!(
                                            "prost decode error: {e}"
                                        )))
                                        .await;
                                    break;
                                }
                            },
                            Err(status) => {
                                let _ = tx_typed.send_err(status).await;
                                break;
                            }
                        }
                    }
                    tx_typed.close();
                });

                let req = Request::from_parts(metadata, extensions, typed_stream);
                let resp = f(svc, req).await?;
                let (metadata, mut out_typed, extensions) = resp.into_parts();

                let (tx_bytes, out_bytes) = stream::channel::<Bytes>(16);
                descartes_tokio::task::spawn_local(async move {
                    while let Some(item) = out_typed.next().await {
                        match item {
                            Ok(msg) => {
                                if tx_bytes
                                    .send(Bytes::from(msg.encode_to_vec()))
                                    .await
                                    .is_err()
                                {
                                    break;
                                }
                            }
                            Err(status) => {
                                let _ = tx_bytes.send_err(status).await;
                                break;
                            }
                        }
                    }
                    tx_bytes.close();
                });

                Ok(Response::from_parts(metadata, out_bytes, extensions))
            }
        })
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
                let (_tx, rx) = descartes_tokio::sync::mpsc::channel(1);
                Ok(Response::new(DesStreaming::new(rx)))
            })
            .add_client_streaming(
                "/svc.C/C",
                |_req| async move { Ok(Response::new(Bytes::new())) },
            )
            .add_bidi_streaming("/svc.B/B", |_req| async move {
                let (_tx, rx) = descartes_tokio::sync::mpsc::channel(1);
                Ok(Response::new(DesStreaming::new(rx)))
            });

        assert_eq!(router.list_unary(), vec!["/svc.U/A".to_string()]);
        assert_eq!(router.list_server_streaming(), vec!["/svc.S/S".to_string()]);
        assert_eq!(router.list_client_streaming(), vec!["/svc.C/C".to_string()]);
        assert_eq!(router.list_bidi_streaming(), vec!["/svc.B/B".to_string()]);
    }
}
