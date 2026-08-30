//! oas-runtime 装配：依赖注入唯一现场（§3.8）+ shim 子命令分发。
//!
//! 单二进制两模式：
//! - `oas-runtime`（无子命令）→ runtime 模式：serve CRI，装配 RedbStore + RealDriver +
//!   真 net/storage + OasManager。runtime 重启不影响已存在的 shim（shim 经 setsid 脱离）。
//! - `oas-runtime shim --config <p> --sandbox-id <id> --socket <uds> --log-file <f>` → shim 模式：
//!   单 VM 守护进程，ttrpc server 等 Create 触发恢复。
//!
//! 依赖方向见各 crate。本文件只做装配 + 模式分发，不含业务逻辑。

use std::path::PathBuf;
use std::sync::Arc;

use clap::{Parser, Subcommand};
use oas_config::Config;
use oas_cri::runtime::v1::image_service_server::ImageServiceServer;
use oas_cri::runtime::v1::runtime_service_server::RuntimeServiceServer;
use oas_cri::{ImageSvc, RuntimeSvc};
use oas_driver::{shim::ShimArgs, RealDriver};
use oas_manager::{Clock, Manager, OasManager, SystemClock, VmReadiness};
use oas_mock::ImmediateReadiness;
use oas_net::NetManager;
use oas_storage::RealStorageManager;
use oas_store::{RedbStore, Store};
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
#[command(version, about = "Only AgentSandbox CRI runtime + shim")]
struct Cli {
    /// 子命令：`shim` 进入 shim 模式；缺省为 runtime 模式。
    #[command(subcommand)]
    mode: Option<Mode>,

    /// 配置文件路径（runtime 与 shim 共用同一份；runtime 透传给 spawn 的 shim）。
    #[arg(long, env = "OAS_CONFIG", default_value = "/etc/oas/config.toml", global = true)]
    config: PathBuf,
    /// 日志文件路径（runtime 模式）。
    #[arg(long, env = "OAS_LOG_FILE")]
    log_file: Option<PathBuf>,
    /// 日志级别：debug|info|warn|error，默认 info（RUST_LOG 优先）。
    #[arg(long, env = "OAS_LOG_LEVEL", default_value = "info")]
    log_level: String,
}

#[derive(Subcommand, Debug)]
enum Mode {
    /// shim 模式：单沙箱守护进程，ttrpc server 等 Create 触发 firecracker snapshot 恢复。
    Shim {
        /// 沙箱句柄（兼作 jailer --id）。
        #[arg(long)]
        sandbox_id: String,
        /// shim ttrpc UDS 路径（runtime 指定）。
        #[arg(long)]
        socket: PathBuf,
        /// shim 日志文件。
        #[arg(long)]
        log_file: PathBuf,
    },
}

/// 初始化分层日志（runtime 模式）。
fn init_logging(log_file: Option<&std::path::Path>, log_level: &str) -> Option<WorkerGuard> {
    let file_filter = match std::env::var("RUST_LOG") {
        Ok(v) if !v.is_empty() => EnvFilter::builder().parse_lossy(v),
        _ => EnvFilter::builder()
            .parse_lossy(format!("h2=warn,hyper=warn,tonic=warn,tokio_util=warn,{log_level}")),
    };
    let stderr_layer = fmt::layer()
        .with_writer(std::io::stderr)
        .with_filter(LevelFilter::WARN);
    let registry = tracing_subscriber::registry().with(stderr_layer);
    if let Some(path) = log_file {
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
    }
}

/// shim 模式日志：写指定 log_file，stderr 兜底 WARN+。
fn init_shim_logging(log_file: &std::path::Path) -> Option<WorkerGuard> {
    let stderr_layer = fmt::layer()
        .with_writer(std::io::stderr)
        .with_filter(LevelFilter::WARN);
    let registry = tracing_subscriber::registry().with(stderr_layer);
    match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_file)
    {
        Ok(file) => {
            let (writer, guard) = tracing_appender::non_blocking(file);
            let file_layer = fmt::layer().with_writer(writer).with_ansi(false);
            registry.with(file_layer).init();
            Some(guard)
        }
        Err(_) => {
            registry.init();
            None
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    match cli.mode {
        // ---- shim 模式 ----
        Some(Mode::Shim {
            sandbox_id,
            socket,
            log_file,
        }) => {
            let _guard = init_shim_logging(&log_file);
            let args = ShimArgs {
                config_path: cli.config,
                sandbox_id,
                socket,
                log_file,
            };
            oas_driver::shim::run(args)?;
            return Ok(());
        }
        // ---- runtime 模式 ----
        None => {}
    }

    let _log_guard = init_logging(cli.log_file.as_deref(), &cli.log_level);

    // ---- 依赖注入：真后端 + 真 Manager + 持久化 Store ----
    let cfg = Arc::new(Config::load(&cli.config));
    let store: Arc<dyn Store> = Arc::new(RedbStore::open(
        cfg.store_path.to_str().ok_or("store_path not utf-8")?,
    )?);
    let driver = Arc::new(RealDriver::new(cli.config.clone()));
    let net = Arc::new(NetManager::new(store.clone(), cfg.clone()));
    let storage = Arc::new(RealStorageManager::new(cfg.clone()));
    let readiness: Arc<dyn VmReadiness> = Arc::new(ImmediateReadiness);
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);

    let mgr: Arc<dyn Manager> = Arc::new(OasManager::new(
        driver,
        net,
        storage,
        store,
        cfg,
        readiness,
        clock,
    ));
    let rt = RuntimeSvc::new(mgr.clone());
    let img = ImageSvc::new(mgr);

    let socket = std::env::var("OAS_SOCKET").unwrap_or_else(|_| "/run/oas.sock".into());
    let _ = std::fs::remove_file(&socket);
    let uds = tokio::net::UnixListener::bind(&socket)?;
    eprintln!("oas-runtime serving CRI on unix:{socket}");

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
