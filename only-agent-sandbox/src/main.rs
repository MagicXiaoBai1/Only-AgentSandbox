//! oas-runtime 装配：依赖注入唯一现场（§3.8）。
//!
//! 当前用 **mock 后端**（`oas_mock` 的 driver/net/storage + `MemoryStore`）装配真 `OasManager`，
//! 让 CRI/Manager/Store 三层在协议层跑通，可对接真 kubelet/crictl 做沙箱级联调。
//!
//! ⚠️ mock 后端不起任何 firecracker 进程、不建 netns/tap、不制备 ext4——沙箱在 CRI 层
//! 显示 READY 但宿主机无真实 VM。真实 `FirecrackerDriver`/`NetworkManager`/`StorageManager`
//! 及 `RedbStore` 就绪后，把这里的构造换成真实现即可（仅本文件改动）。

use std::sync::Arc;

use oas_cri::runtime::v1::image_service_server::ImageServiceServer;
use oas_cri::runtime::v1::runtime_service_server::RuntimeServiceServer;
use oas_cri::{ImageSvc, RuntimeSvc};
use oas_manager::{Clock, Manager, OasManager, SystemClock, VmReadiness};
use oas_mock::{ImmediateReadiness, MockDriver, MockNet, MockStorage};
use oas_store::{MemoryStore, Store};
use tonic::transport::Server;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 日志：RUST_LOG 控制；tracing/log 桥让 h2/hyper/tonic 的事件流到 env_logger。
    let _ = env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("warn"),
    )
    .try_init();

    // ---- 依赖注入：mock 后端 + 真 Manager + 真 Store ----
    let store: Arc<dyn Store> = Arc::new(MemoryStore::new());
    let driver = Arc::new(MockDriver::new());
    let net = Arc::new(MockNet::new(
        store.clone(),
        "10.244.0.0/24",
        "10.244.0.254",
    ));
    let storage = Arc::new(MockStorage::new());
    let readiness: Arc<dyn VmReadiness> = Arc::new(ImmediateReadiness);
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);

    let mgr: Arc<dyn Manager> = Arc::new(OasManager::new(
        driver,
        net,
        storage,
        store,
        readiness,
        clock,
    ));
    let rt = RuntimeSvc::new(mgr.clone());
    let img = ImageSvc::new(mgr);

    let socket = std::env::var("OAS_SOCKET").unwrap_or_else(|_| "/run/oas.sock".into());
    let _ = std::fs::remove_file(&socket);
    let uds = tokio::net::UnixListener::bind(&socket)?;
    eprintln!(
        "oas-runtime serving CRI on unix:{socket} (MOCK backend: no real firecracker/netns/ext4)"
    );

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
