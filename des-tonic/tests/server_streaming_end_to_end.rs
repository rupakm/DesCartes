use bytes::Bytes;
use des_core::{Execute, Executor, SimTime, Simulation};
use des_tonic::{ClientBuilder, DesStreaming, Router, ServerBuilder, Transport};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tonic::{Request, Response, Status};

#[test]
fn server_streaming_end_to_end_via_server_endpoint() {
    std::thread::spawn(|| {
        let mut sim = Simulation::default();
        des_tokio::runtime::install(&mut sim);

        let transport = Transport::install_default(&mut sim);

        let mut router = Router::new();
        router.add_server_streaming("/svc.Test/Download", |req: Request<Bytes>| async move {
            let req_payload = req.into_inner();
            let (tx, rx) = des_tokio::sync::mpsc::channel::<Result<Bytes, Status>>(8);

            des_tokio::task::spawn_local(async move {
                // Emit three items then end.
                let _ = tx.send(Ok(req_payload.clone())).await;
                let _ = tx.send(Ok(Bytes::from_static(b":chunk1"))).await;
                let _ = tx.send(Ok(Bytes::from_static(b":chunk2"))).await;
            });

            Ok::<_, Status>(Response::new(DesStreaming::new(rx)))
        });

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

        let channel = installed.channel;

        let result: Arc<Mutex<Option<Result<Vec<Bytes>, Status>>>> = Arc::new(Mutex::new(None));
        let result_out = result.clone();

        des_tokio::task::spawn_local(async move {
            let resp = channel
                .server_streaming(
                    "/svc.Test/Download",
                    Request::new(Bytes::from_static(b"hello")),
                    Some(Duration::from_millis(200)),
                )
                .await;

            let out = match resp {
                Ok(resp) => {
                    let mut stream = resp.into_inner();
                    let mut items = Vec::new();
                    let mut err: Option<Status> = None;

                    while let Some(item) = stream.next().await {
                        match item {
                            Ok(b) => items.push(b),
                            Err(e) => {
                                err = Some(e);
                                break;
                            }
                        }
                    }

                    match err {
                        Some(e) => Err(e),
                        None => Ok(items),
                    }
                }
                Err(e) => Err(e),
            };

            *result_out.lock().unwrap() = Some(out);
        });

        Executor::timed(SimTime::from_millis(50)).execute(&mut sim);

        let out = result.lock().unwrap().take().expect("result set").unwrap();
        assert_eq!(out.len(), 3);
        assert_eq!(out[0], Bytes::from_static(b"hello"));
        assert_eq!(out[1], Bytes::from_static(b":chunk1"));
        assert_eq!(out[2], Bytes::from_static(b":chunk2"));
    })
    .join()
    .unwrap();
}
