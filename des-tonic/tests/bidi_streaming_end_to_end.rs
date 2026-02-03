use bytes::Bytes;
use descartes_core::{Execute, Executor, SimTime, Simulation};
use descartes_tonic::{ClientBuilder, DesStreaming, Router, ServerBuilder, Transport};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tonic::{Request, Response, Status};

#[test]
fn bidi_streaming_end_to_end_via_server_endpoint() {
    std::thread::spawn(|| {
        let mut sim = Simulation::default();
        descartes_tokio::runtime::install(&mut sim);

        let transport = Transport::install_default(&mut sim);

        let mut router = Router::new();
        router.add_bidi_streaming(
            "/svc.Test/Chat",
            |req: Request<DesStreaming<Bytes>>| async move {
                let mut inbound = req.into_inner();
                let (tx, rx) = descartes_tokio::sync::mpsc::channel::<Result<Bytes, Status>>(16);

                descartes_tokio::task::spawn_local(async move {
                    while let Some(item) = inbound.next().await {
                        match item {
                            Ok(bytes) => {
                                // Echo back with a prefix.
                                let mut out = Bytes::from_static(b"echo:").to_vec();
                                out.extend_from_slice(&bytes);
                                let _ = tx.send(Ok(Bytes::from(out))).await;
                            }
                            Err(status) => {
                                let _ = tx.send(Err(status)).await;
                                break;
                            }
                        }
                    }
                });

                Ok::<_, Status>(Response::new(DesStreaming::new(rx)))
            },
        );

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

        let result: Arc<Mutex<Option<Vec<Bytes>>>> = Arc::new(Mutex::new(None));
        let result_out = result.clone();

        descartes_tokio::task::spawn_local(async move {
            let (sender, resp) = channel
                .bidirectional_streaming("/svc.Test/Chat", Some(Duration::from_millis(200)))
                .await
                .unwrap();

            // Send 3 messages and close.
            sender.send(Bytes::from_static(b"a")).await.unwrap();
            sender.send(Bytes::from_static(b"b")).await.unwrap();
            sender.send(Bytes::from_static(b"c")).await.unwrap();
            sender.close();

            let mut stream = resp.into_inner();
            let mut items = Vec::new();
            while let Some(item) = stream.next().await {
                items.push(item.unwrap());
                if items.len() == 3 {
                    break;
                }
            }

            *result_out.lock().unwrap() = Some(items);
        });

        Executor::timed(SimTime::from_millis(100)).execute(&mut sim);

        let out = result.lock().unwrap().take().expect("result set");
        assert_eq!(
            out,
            vec![
                Bytes::from_static(b"echo:a"),
                Bytes::from_static(b"echo:b"),
                Bytes::from_static(b"echo:c"),
            ]
        );
    })
    .join()
    .unwrap();
}
