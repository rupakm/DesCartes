use des_core::{Execute, Executor, SimTime, Simulation};
use des_tonic::{ClientBuilder, DesStreaming, Router, ServerBuilder, Transport};
use std::sync::{Arc, Mutex};
use tonic::{Request, Response, Status};

use des_tonic_codegen_test::pb;

struct FooImpl;

#[tonic::async_trait]
impl pb::foo_server::Foo for FooImpl {
    async fn unary(&self, request: Request<pb::Num>) -> Result<Response<pb::Num>, Status> {
        let v = request.into_inner().v;
        Ok(Response::new(pb::Num { v: v + 1 }))
    }

    async fn server_stream(
        &self,
        request: Request<pb::Num>,
    ) -> Result<Response<DesStreaming<pb::Num>>, Status> {
        let n = request.into_inner().v;
        let (tx, rx) = des_tokio::sync::mpsc::channel::<Result<pb::Num, Status>>(8);

        des_tokio::task::spawn_local(async move {
            for i in 0..n {
                if tx.send(Ok(pb::Num { v: i })).await.is_err() {
                    return;
                }
            }
        });

        Ok(Response::new(DesStreaming::new(rx)))
    }

    async fn client_stream(
        &self,
        request: Request<DesStreaming<pb::Num>>,
    ) -> Result<Response<pb::Num>, Status> {
        let mut inbound = request.into_inner();
        let mut sum = 0i32;

        while let Some(item) = inbound.next().await {
            sum += item?.v;
        }

        Ok(Response::new(pb::Num { v: sum }))
    }

    async fn bidi(
        &self,
        request: Request<DesStreaming<pb::Num>>,
    ) -> Result<Response<DesStreaming<pb::Num>>, Status> {
        let mut inbound = request.into_inner();
        let (tx, rx) = des_tokio::sync::mpsc::channel::<Result<pb::Num, Status>>(8);

        des_tokio::task::spawn_local(async move {
            while let Some(item) = inbound.next().await {
                match item {
                    Ok(num) => {
                        if tx.send(Ok(pb::Num { v: num.v * 2 })).await.is_err() {
                            return;
                        }
                    }
                    Err(status) => {
                        let _ = tx.send(Err(status)).await;
                        return;
                    }
                }
            }
        });

        Ok(Response::new(DesStreaming::new(rx)))
    }
}

#[test]
fn codegen_end_to_end() {
    std::thread::spawn(|| {
        let mut sim = Simulation::default();
        des_tokio::runtime::install(&mut sim);

        let transport = Transport::install_default(&mut sim);

        let mut router = Router::new();
        pb::foo_server::FooServer::new(FooImpl).register(&mut router);

        let _server = ServerBuilder::new(
            "svc".to_string(),
            "server".to_string(),
            transport.transport_key,
            transport.endpoint_registry.clone(),
            transport.scheduler.clone(),
        )
        .add_router(router)
        .install(&mut sim)
        .unwrap();

        let installed = ClientBuilder::new(
            "svc".to_string(),
            transport.transport_key,
            transport.endpoint_registry.clone(),
            transport.scheduler.clone(),
        )
        .client_name("c1")
        .install(&mut sim)
        .unwrap();

        let client = pb::foo_client::FooClient::new(installed.channel);

        let unary_out: Arc<Mutex<Option<i32>>> = Arc::new(Mutex::new(None));
        let srv_stream_out: Arc<Mutex<Option<Vec<i32>>>> = Arc::new(Mutex::new(None));
        let cli_stream_out: Arc<Mutex<Option<i32>>> = Arc::new(Mutex::new(None));
        let bidi_out: Arc<Mutex<Option<Vec<i32>>>> = Arc::new(Mutex::new(None));

        let unary_out_task = unary_out.clone();
        let srv_stream_out_task = srv_stream_out.clone();
        let cli_stream_out_task = cli_stream_out.clone();
        let bidi_out_task = bidi_out.clone();

        des_tokio::task::spawn_local(async move {
            let client = client;
            let unary = client.unary(pb::Num { v: 41 }).await.unwrap().into_inner();
            *unary_out_task.lock().unwrap() = Some(unary.v);

            let mut stream = client
                .server_stream(pb::Num { v: 3 })
                .await
                .unwrap()
                .into_inner();
            let mut items = Vec::new();
            while let Some(item) = stream.next().await {
                items.push(item.unwrap().v);
            }
            *srv_stream_out_task.lock().unwrap() = Some(items);

            let (sender, resp_fut) = client.client_stream().await.unwrap();
            sender.send(pb::Num { v: 1 }).await.unwrap();
            sender.send(pb::Num { v: 2 }).await.unwrap();
            sender.send(pb::Num { v: 3 }).await.unwrap();
            drop(sender);
            let sum = resp_fut.await.unwrap().into_inner().v;
            *cli_stream_out_task.lock().unwrap() = Some(sum);

            let (sender, resp) = client.bidi().await.unwrap();
            sender.send(pb::Num { v: 7 }).await.unwrap();
            sender.send(pb::Num { v: 9 }).await.unwrap();
            drop(sender);

            let mut stream = resp.into_inner();
            let mut items = Vec::new();
            while let Some(item) = stream.next().await {
                items.push(item.unwrap().v);
                if items.len() == 2 {
                    break;
                }
            }
            *bidi_out_task.lock().unwrap() = Some(items);
        });

        Executor::timed(SimTime::from_millis(200)).execute(&mut sim);

        assert_eq!(unary_out.lock().unwrap().take(), Some(42));
        assert_eq!(srv_stream_out.lock().unwrap().take(), Some(vec![0, 1, 2]));
        assert_eq!(cli_stream_out.lock().unwrap().take(), Some(6));
        assert_eq!(bidi_out.lock().unwrap().take(), Some(vec![14, 18]));
    })
    .join()
    .unwrap();
}
