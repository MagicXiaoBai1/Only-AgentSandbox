//! containerd-shim-oas-v2 — CLI 分发 (Legacy start 协议, 对齐 containerd 2.1).
//!
//! containerd 按 v2 shim 约定调用本二进制:
//!   - `<bin> -namespace <ns> -id <id> -address <ctrd.sock> -publish-binary <p> start`
//!     计算 ttrpc socket 地址, spawn 一个常驻 `run` 子进程 (setsid 脱离),
//!     把 socket 地址打到 stdout (Legacy 协议: JSON/纯地址串), 自身退出.
//!   - `<bin> ... -socket <addr> [run]` (无 start/delete) → 常驻: bind socket + serve
//!     Sandbox(+Task) service, 阻塞至 ShutdownSandbox 触发退出.
//!   - `<bin> ... delete` → 应急清理 (P1 mock 下退: 无进程可杀, 仅回默认 DeleteResponse).
//!   - `<bin> -v` : 版本; `<bin> -info` > RuntimeInfo protobuf (containerd 2.x 探测用).
//!
//! containerd-shim 0.11 的 `run()` 只启动 Task service, 不挂 Sandbox; 故这里手动
//! 用 `server::build_and_start` 注册 Sandbox+Task (见清单#1 / kata 同款手动 register).

use std::io::Write;
use std::path::Path;
use std::sync::Arc;

use containerd_shim as shim;
use containerd_shim_protos::protobuf::Message;
use containerd_shim_protos::types::introspection::{RuntimeInfo, RuntimeVersion};

use oas_config::Config;
use oas_ctrd_shim::common::server;
use oas_ctrd_shim::common::start;
use oas_ctrd_shim::common::vm::{MockVm, RealVm, SandboxVm};

const RUNTIME_ID: &str = "io.containerd.oas.v2";
const RUNTIME_VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() {
    let os_args: Vec<std::ffi::OsString> = std::env::args_os().collect();
    let flags = match shim::parse(&os_args[1..]) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("containerd-shim-oas-v2: parse args: {e}");
            std::process::exit(1);
        }
    };

    // -v / --version
    if flags.version {
        println!(
            "containerd-shim-oas-v2:\n  runtime: {RUNTIME_ID}\n  version: {RUNTIME_VERSION}"
        );
        return;
    }

    // -info: 输出 RuntimeInfo protobuf (containerd 2.x 探测).
    if flags.info {
        if let Err(e) = show_info() {
            eprintln!("containerd-shim-oas-v2: info: {e}");
            std::process::exit(1);
        }
        return;
    }

    match flags.action.as_str() {
        "start" => {
            // grouping 按协议统一分发 (见 start::resolve_grouping / ADR 决策4):
            // Container+Task 落到 pod 的 shim (sandbox-id), 其余用 flags.id (含 2.x 路径).
            let grouping = start::resolve_grouping(&flags);
            if let Err(e) = start::action_start(&flags, &grouping) {
                eprintln!("containerd-shim-oas-v2: start: {e}");
                std::process::exit(1);
            }
        }

        "delete" => {
            // containerd 在 shim 不可达时调本二进制 delete 做应急回收。
            // 真下层: 走 oas-driver emergency_kill（读 shim.meta → 身份校验 → 杀 fc → 清 jail root/socket）。
            let cfg = load_config();
            let sid = flags.id.as_str();
            if !sid.is_empty() {
                let sandbox_dir = cfg.sandbox_dir(sid);
                let jail_root = cfg.jail_root(sid);
                let socket = cfg.shim_socket(sid);
                if let Err(e) = oas_driver::emergency_kill(&sandbox_dir, &jail_root, &socket) {
                    eprintln!("containerd-shim-oas-v2: delete emergency_kill: {e}");
                }
            }
            let resp = containerd_shim_protos::api::DeleteResponse::new();
            if let Ok(bytes) = resp.write_to_bytes() {
                let _ = std::io::stdout().write_all(&bytes);
            }
        }

        // 空 action 或 "run": 常驻 serve.
        _ => {
            if flags.socket.is_empty() {
                eprintln!("containerd-shim-oas-v2: run: -socket cannot be empty");
                std::process::exit(1);
            }

            if let Err(e) = run_server(&flags.socket) {
                eprintln!("containerd-shim-oas-v2: run: {e}");
                std::process::exit(1);
            }
        }
    }
}

/// 常驻 server: bind socket + serve Sandbox(+Task), 阻塞至 ShutdownSandbox.
fn run_server(socket_addr: &str) -> Result<(), Box<dyn std::error::Error>> {
    // 安装 tracing 订阅器：oas-driver 的 log_step (target="oas-shim") 各阶段耗时经此落到
    // stderr。常驻 run 子进程的 stderr 由 action_start 重定向到 OAS_SHIM_LOG 文件（若未设
    // 则 /dev/null）。EnvFilter 读 RUST_LOG；未设时回落 info（含 log_step 的 info 级）。
    init_tracing();
    // 诊断探针：与 log_step 同形。走 stderr(=OAS_SHIM_LOG 文件)，不污染 stdout 握手。
    // 若它在日志里出现 → 订阅器写文件成功；log_step 也应出现，否则问题在 create 调用链。
    tracing::info!(target: "oas-shim", sid = "boot", step = "tracing_probe", elapsed_ms = 0u64, "restore step");

    let cfg = Arc::new(load_config());
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;

    rt.block_on(async move {
        let vm = make_vm(cfg);
        let (mut server, exit) = server::build_and_start(socket_addr, vm).await?;

        // socket 已 bind + serve 就绪: 向 stdout 报地址, 然后把 stdout 重定向到
        // /dev/null (关闭 pipe → 父进程 action_start 读到 EOF). 这是与父进程的就绪握手.
        start::signal_ready(socket_addr);

        // 阻塞至 ShutdownSandbox 触发 exit.
        exit.wait().await;
        server.shutdown().await.ok();
        Ok::<(), Box<dyn std::error::Error>>(())
    })
}

/// 安装 tracing fmt 订阅器（写 stderr，由 action_start 重定向到 OAS_SHIM_LOG 文件）。
///
/// oas-driver `vm_core::log_step` 用 `tracing::info!(target="oas-shim", step=.., elapsed_ms=..)`
/// 标注恢复各阶段耗时；本进程不装订阅器时这些事件经 tracing "log" feature 回落到 `log` crate，
/// 而 shim 又未注册 `log` logger → 全部丢弃。装上订阅器后事件直接进订阅器（不再回落）。
fn init_tracing() {
    use tracing_subscriber::{fmt, EnvFilter};
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    // 默认 writer 是 stdout——会污染 containerd Legacy start 握手（stdout 读 socket 地址），
    // 且 signal_ready 后 stdout→/dev/null 会丢失 create/start 期间的 log_step 事件。
    // 显式写 stderr：action_start 已把常驻子进程 stderr 重定向到 OAS_SHIM_LOG 文件。
    let _ = fmt()
        .with_env_filter(filter)
        .with_target(true)
        .with_writer(std::io::stderr)
        .try_init();
}

/// 解析配置: `OAS_CONFIG` 环境变量 → `/etc/oas/config.toml` → `Config::default()`。
///
/// containerd 经 Legacy start 协议拉起 shim, `shim::parse` 不透传 `--config`（见 ADR 0010）,
/// 故走 env + 固定路径。`Config::load` 本身对文件缺失回落 Default, 故两路径都覆盖。
fn load_config() -> Config {
    if let Ok(p) = std::env::var("OAS_CONFIG") {
        return Config::load(Path::new(&p));
    }
    Config::load(Path::new("/etc/oas/config.toml"))
}

/// 造下层 VM: 默认 `RealVm`（真 firecracker restore）; `OAS_VM=mock` 退回 `MockVm`
/// 供无 firecracker/bundle 环境下的协议级 e2e 复用。
fn make_vm(cfg: Arc<Config>) -> Arc<dyn SandboxVm> {
    if std::env::var("OAS_VM").as_deref() == Ok("mock") {
        Arc::new(MockVm::new())
    } else {
        Arc::new(RealVm::new(cfg))
    }
}

/// 输出 RuntimeInfo protobuf 到 stdout (containerd 2.x `-info` 探测).
fn show_info() -> Result<(), Box<dyn std::error::Error>> {
    let mut version = RuntimeVersion::new();
    version.version = RUNTIME_VERSION.to_string();

    let mut info = RuntimeInfo::new();
    info.name = RUNTIME_ID.to_string();
    info.version = protobuf::MessageField::some(version);

    let bytes = info.write_to_bytes()?;
    std::io::stdout().write_all(&bytes)?;
    Ok(())
}