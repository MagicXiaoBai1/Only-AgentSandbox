//! oas-runtime 装配入口（§4 main.rs，依赖注入唯一现场）。
//!
//! 当前注入 `StubManager`（§3.2 真 Manager 落地前），起 UDS serve。
//! 握手类（Version/Status/ListImages/ImageFsInfo）真实可用；生命周期类经 stub
//! 优雅报 `UNAVAILABLE`，待 §3.2 换真 Manager 时只改这里注入。

use std::sync::Arc;

use oas_cri::runtime::v1::image_service_server::ImageServiceServer;
use oas_cri::runtime::v1::runtime_service_server::RuntimeServiceServer;
use oas_cri::{ImageSvc, RuntimeSvc};
use oas_manager::{Manager, StubManager};
use tonic::transport::Server;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mgr: Arc<dyn Manager> = Arc::new(StubManager::new());
    let rt = RuntimeSvc::new(mgr.clone());
    let img = ImageSvc::new(mgr);

    let socket = std::env::var("OAS_SOCKET").unwrap_or_else(|_| "/run/oas.sock".into());
    // 清理残留 socket 文件。
    let _ = std::fs::remove_file(&socket);

    let uds = tokio::net::UnixListener::bind(&socket)?;
    eprintln!("oas-runtime serving CRI on unix:{socket}");

    // 官方 tonic UDS 模式：yield Result<UnixStream, io::Error> 喂 serve_with_incoming。
    let incoming = async_stream::stream! {
        loop {
            match uds.accept().await {
                Ok((stream, _)) => yield Ok::<_, std::io::Error>(stream),
                Err(e) => {
                    eprintln!("accept error: {e}");
                    continue;
                }
            }
        }
    };

    Server::builder()
        .add_service(RuntimeServiceServer::new(rt))
        .add_service(ImageServiceServer::new(img))
        .serve_with_incoming(incoming)
        .await?;

    Ok(())
}
