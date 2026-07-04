//! oas-runtime 装配：依赖注入唯一现场（§3.8）。
//!
//! 当前用 **mock 后端**（`oas_mock` 的 driver/net/storage + `MemoryStore`）装配真 `OasManager`，
//! 让 CRI/Manager/Store 三层在协议层跑通，可对接真 kubelet/crictl 做沙箱级联调。
//!
//! ⚠️ mock 后端不起任何 firecracker 进程、不建 netns/tap、不制备 ext4——沙箱在 CRI 层
//! 显示 READY 但宿主机无真实 VM。真实 `FirecrackerDriver`/`NetworkManager`/`StorageManager`
//! 及 `RedbStore` 就绪后，把这里的构造换成真实现即可（仅本文件改动）。

use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use oas_cri::runtime::v1::image_service_server::ImageServiceServer;
use oas_cri::runtime::v1::runtime_service_server::RuntimeServiceServer;
use oas_cri::{ImageSvc, RuntimeSvc};
use oas_manager::{Clock, Manager, OasManager, SystemClock, VmReadiness};
use oas_mock::{ImmediateReadiness, MockDriver, MockNet, MockStorage};
use oas_store::{MemoryStore, Store};
use tonic::transport::Server;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::fmt;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::Layer;
use tracing_subscriber::EnvFilter;

/// oas-runtime 启动参数。
#[derive(Parser, Debug)]
#[command(version, about = "Only AgentSandbox CRI runtime")]
struct Cli {
    /// 日志文件路径；未指定则不写文件，仅输出到 stderr。
    #[arg(long, env = "OAS_LOG_FILE")]
    log_file: Option<PathBuf>,
    /// 日志级别：debug|info|warn|error，默认 info（RUST_LOG 优先于本参数）。
    #[arg(long, env = "OAS_LOG_LEVEL", default_value = "info")]
    log_level: String,
}

/// 初始化分层日志。
///
/// - 文件层（`log_file` 存在时）：按 `log_level` 写全量，非阻塞，返回的 guard 必须由调用方
///   持有至程序退出，以保证落盘 flush。
/// - stderr 层：固定 WARN 及以上。
///
/// `RUST_LOG` 若设置则作为文件层的 EnvFilter（高级 per-module 过滤），否则按 `log_level`
/// 构造默认指令并让 h2/hyper/tonic 等依赖保持 warn。
fn init_logging(log_file: Option<&std::path::Path>, log_level: &str) -> Option<WorkerGuard> {
    // tracing-subscriber 启用 tracing-log feature 后，init() 会自动调用 LogTracer::init()，
    // 把 log crate 事件（h2/hyper/tonic）桥接到本订阅器，无需手动再调。

    let file_filter = match std::env::var("RUST_LOG") {
        Ok(v) if !v.is_empty() => EnvFilter::builder().parse_lossy(v),
        _ => EnvFilter::builder()
            .parse_lossy(format!("h2=warn,hyper=warn,tonic=warn,tokio_util=warn,{log_level}")),
    };

    let stderr_layer = fmt::layer()
        .with_writer(std::io::stderr)
        .with_filter(LevelFilter::WARN);

    let registry = tracing_subscriber::registry().with(stderr_layer);

    let guard = if let Some(path) = log_file {
        match std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            Ok(file) => {
                let (writer, guard) = tracing_appender::non_blocking(file);
                let file_layer = fmt::layer()
                    .with_writer(writer)
                    .with_ansi(false)
                    .with_filter(file_filter);
                // 这里需要先把 file_layer 装进去再返回 guard；用 once cell 风格不便，
                // 故直接在分支内完成 init 并返回 guard。
                registry.with(file_layer).init();
                Some(guard)
            }
            Err(e) => {
                eprintln!("oas-runtime: failed to open log file {path:?}: {e}; falling back to stderr only");
                registry.init();
                None
            }
        }
    } else {
        registry.init();
        None
    };

    guard
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    let _log_guard = init_logging(cli.log_file.as_deref(), &cli.log_level);

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
                    tracing::error!("accept error: {e}");
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
