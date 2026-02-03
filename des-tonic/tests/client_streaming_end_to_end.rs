use bytes::Bytes;
use descartes_core::{Execute, Executor, SimTime, Simulation};
use descartes_tonic::{ClientBuilder, Router, ServerBuilder, Transport};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tonic::{Request, Response, Status};

#[test]
fn client_streaming_end_to_end_via_server_endpoint() {
    std::thread::spawn(|| {
        let mut sim = Simulation::default();
        descartes_tokio::runtime::install(&mut sim);

        let transport = Transport::install_default(&mut sim);

        let mut router = Router::new();
        router.add_client_streaming(
            "/svc.Test/Upload",
            |req: Request<descartes_tonic::DesStreaming<Bytes>>| async move {
                let mut stream = req.into_inner();
                let mut out = Vec::new();

                while let Some(item) = stream.next().await {
                    let bytes = item?;
                    out.extend_from_slice(&bytes);
                }

                Ok::<_, Status>(Response::new(Bytes::from(out)))
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

        let result: Arc<Mutex<Option<Result<Bytes, Status>>>> = Arc::new(Mutex::new(None));
        let result_out = result.clone();

        descartes_tokio::task::spawn_local(async move {
            let (sender, resp) = channel
                .client_streaming("/svc.Test/Upload", Some(Duration::from_millis(200)))
                .await
                .unwrap();

            sender.send(Bytes::from_static(b"hello")).await.unwrap();
            sender.send(Bytes::from_static(b"-")).await.unwrap();
            sender.send(Bytes::from_static(b"world")).await.unwrap();
            sender.close();

            let resp = resp.await.map(|r| r.into_inner());
            *result_out.lock().unwrap() = Some(resp);
        });

        Executor::timed(SimTime::from_millis(50)).execute(&mut sim);

        let out = result.lock().unwrap().take().expect("result set");
        assert_eq!(out.unwrap(), Bytes::from_static(b"hello-world"));
    })
    .join()
    .unwrap();
}
