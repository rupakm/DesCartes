use std::sync::Arc;
use std::time::Duration;

use des_core::{Execute, Executor, SimTime, Simulation, SimulationConfig};
use des_tonic::{stream, Channel, ClientBuilder, DesStreaming, Router, ServerBuilder, Transport};
use tonic::{Request, Response, Status};

const METHOD_CHAT: &str = "/chat.Chat/Chat";

#[derive(Clone, PartialEq, prost::Message)]
pub struct ChatMessage {
    #[prost(string, tag = "1")]
    pub user: String,
    #[prost(string, tag = "2")]
    pub text: String,
}

#[des_tonic::async_trait]
pub trait Chat: Send + Sync + 'static {
    async fn chat(
        &self,
        request: Request<DesStreaming<ChatMessage>>,
    ) -> Result<Response<DesStreaming<ChatMessage>>, Status>;
}

#[derive(Clone)]
pub struct ChatServer<T> {
    inner: Arc<T>,
}

impl<T: Chat> ChatServer<T> {
    pub fn new(inner: T) -> Self {
        Self {
            inner: Arc::new(inner),
        }
    }

    pub fn register(self, router: &mut Router) {
        router.add_bidi_streaming_prost(
            METHOD_CHAT,
            self.inner.clone(),
            |svc: Arc<T>, req| async move { svc.chat(req).await },
        );
    }
}

#[derive(Clone)]
pub struct ChatClient {
    inner: Channel,
    timeout: Option<Duration>,
}

impl ChatClient {
    pub fn new(inner: Channel) -> Self {
        Self {
            inner,
            timeout: None,
        }
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    pub async fn chat(
        &self,
    ) -> Result<(stream::Sender<ChatMessage>, DesStreaming<ChatMessage>), Status> {
        let (tx, resp) = self
            .inner
            .bidirectional_streaming_prost::<ChatMessage, ChatMessage>(METHOD_CHAT, self.timeout)
            .await?;

        Ok((tx, resp.into_inner()))
    }
}

#[derive(Default)]
struct ChatImpl;

#[des_tonic::async_trait]
impl Chat for ChatImpl {
    async fn chat(
        &self,
        request: Request<DesStreaming<ChatMessage>>,
    ) -> Result<Response<DesStreaming<ChatMessage>>, Status> {
        let mut inbound = request.into_inner();
        let (out_tx, out_rx) = stream::channel::<ChatMessage>(16);

        // Simple echo server: respond with "server: echo:<text>".
        des_tokio::task::spawn_local(async move {
            while let Some(item) = inbound.next().await {
                match item {
                    Ok(msg) => {
                        let reply = ChatMessage {
                            user: "server".to_string(),
                            text: format!("echo:{}", msg.text),
                        };
                        if out_tx.send(reply).await.is_err() {
                            break;
                        }
                    }
                    Err(status) => {
                        let _ = out_tx.send_err(status).await;
                        break;
                    }
                }
            }
            out_tx.close();
        });

        Ok(Response::new(out_rx))
    }
}

fn main() {
    let mut sim = Simulation::new(SimulationConfig { seed: 1 });
    des_tokio::runtime::install(&mut sim);

    let transport = Transport::install_default(&mut sim);

    // Server.
    let mut router = Router::new();
    ChatServer::new(ChatImpl::default()).register(&mut router);

    let _server = ServerBuilder::new(
        "chat".to_string(),
        "server".to_string(),
        transport.transport_key,
        transport.endpoint_registry.clone(),
        transport.scheduler.clone(),
    )
    .add_router(router)
    .install(&mut sim)
    .unwrap();

    // Client.
    let installed = ClientBuilder::new(
        "chat".to_string(),
        transport.transport_key,
        transport.endpoint_registry.clone(),
        transport.scheduler.clone(),
    )
    .client_name("alice")
    .install(&mut sim)
    .unwrap();

    let client = ChatClient::new(installed.channel).with_timeout(Duration::from_millis(200));

    des_tokio::task::spawn_local(async move {
        let (tx, mut rx) = client.chat().await.unwrap();

        // Send a few messages.
        tx.send(ChatMessage {
            user: "alice".to_string(),
            text: "hi".to_string(),
        })
        .await
        .unwrap();

        tx.send(ChatMessage {
            user: "alice".to_string(),
            text: "anyone there?".to_string(),
        })
        .await
        .unwrap();

        tx.close();

        // Read responses.
        while let Some(item) = rx.next().await {
            let msg = item.unwrap();
            println!("{}: {}", msg.user, msg.text);
        }
    });

    Executor::timed(SimTime::from_millis(200)).execute(&mut sim);
}
