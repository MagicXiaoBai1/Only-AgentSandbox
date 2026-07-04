//! 端到端：在真实 UDS 上跑 CRI 握手 + 生命周期短路（StubManager）。

use std::sync::Arc;

use oas_cri::runtime::v1::image_service_client::ImageServiceClient;
use oas_cri::runtime::v1::image_service_server::ImageServiceServer;
use oas_cri::runtime::v1::runtime_service_client::RuntimeServiceClient;
use oas_cri::runtime::v1::runtime_service_server::RuntimeServiceServer;
use oas_cri::runtime::v1::{
    ImageFsInfoRequest, ListContainerStatsRequest, ListImagesRequest, PodSandboxConfig, PodSandboxMetadata,
    RunPodSandboxRequest, StatusRequest, VersionRequest,
};
use oas_cri::{ImageSvc, RuntimeSvc};
use oas_manager::{Manager, StubManager};
use tonic::transport::{Endpoint, Server};

async fn serve(path: &str) -> tokio::task::JoinHandle<()> {
    let mgr: Arc<dyn Manager> = Arc::new(StubManager::new());
    let rt = RuntimeSvc::new(mgr.clone());
    let img = ImageSvc::new(mgr);
    let uds = tokio::net::UnixListener::bind(path).unwrap();
    let incoming = async_stream::stream! {
        loop {
            match uds.accept().await {
                Ok((s, _)) => yield Ok::<_, std::io::Error>(s),
                Err(_) => continue,
            }
        }
    };
    tokio::spawn(async move {
        let _ = Server::builder()
            .add_service(RuntimeServiceServer::new(rt))
            .add_service(ImageServiceServer::new(img))
            .serve_with_incoming(incoming)
            .await;
    })
}

#[tokio::test]
async fn handshake_and_stub_short_circuit_over_uds() {
    let path = format!("/tmp/oas-cri-test-{}.sock", std::process::id());
    let _ = std::fs::remove_file(&path);
    let handle = serve(&path).await;

    let channel = Endpoint::from_shared(format!("unix:{path}"))
        .unwrap()
        .connect()
        .await
        .unwrap();
    let mut rt = RuntimeServiceClient::new(channel.clone());
    let mut img = ImageServiceClient::new(channel);

    // Version：固定返回。
    let v = rt
        .version(VersionRequest {
            version: "0.1.0".into(),
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(v.runtime_name, "only-agent-sandbox");
    assert_eq!(v.runtime_api_version, "v1");

    // Status：固定两 condition=true。
    let s = rt
        .status(StatusRequest::default())
        .await
        .unwrap()
        .into_inner();
    let conditions = s.status.unwrap().conditions;
    assert_eq!(conditions.len(), 2);
    assert!(
        conditions
            .iter()
            .any(|c| c.r#type == "RuntimeReady" && c.status)
    );
    assert!(
        conditions
            .iter()
            .any(|c| c.r#type == "NetworkReady" && c.status)
    );

    // ListImages：白名单空。
    let imgs = img
        .list_images(ListImagesRequest::default())
        .await
        .unwrap()
        .into_inner();
    assert!(imgs.images.is_empty());

    // ImageFsInfo：假“磁盘很空”值。
    let fs = img
        .image_fs_info(ImageFsInfoRequest::default())
        .await
        .unwrap()
        .into_inner();
    assert!(!fs.image_filesystems.is_empty());
    assert!(fs.image_filesystems[0].timestamp > 0);
    assert_eq!(
        fs.image_filesystems[0].fs_id.as_ref().unwrap().mountpoint,
        "/"
    );

    assert!(fs.container_filesystems.is_empty());

    // ListContainerStats: 空 store 下返回空列表，而不是 Unimplemented。
    let stats = rt
        .list_container_stats(ListContainerStatsRequest::default())
        .await
        .unwrap()
        .into_inner();

    assert!(stats.stats.is_empty());
    
    // RunPodSandbox（带合法 type 注解）→ stub 返回 UNAVAILABLE。
    let cfg = PodSandboxConfig {
        metadata: Some(PodSandboxMetadata {
            name: "n".into(),
            uid: "u".into(),
            namespace: "ns".into(),
            attempt: 0,
        }),
        annotations: [("agent-sandbox/type".to_string(), "0".to_string())]
            .into_iter()
            .collect(),
        ..Default::default()
    };
    let err = rt
        .run_pod_sandbox(RunPodSandboxRequest {
            config: Some(cfg),
            runtime_handler: String::new(),
        })
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::Unavailable);

    handle.abort();
    let _ = std::fs::remove_file(&path);
}
