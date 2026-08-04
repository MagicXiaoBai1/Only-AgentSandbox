//! v2 shim start 握手原语 (协议无关)。
//!
//! containerd 按 v2 shim 约定调 `<bin> ... start`: 本进程计算 ttrpc socket 地址、
//! spawn 常驻 `run` 子进程 (setsid 脱离), 阻塞读子进程 stdout 到 EOF 拿就绪信号,
//! 再把地址打到自身 stdout, 子进程 bind socket 成功后写地址 + 关 stdout (EOF)。
//!
//! 归组 (grouping) 随协议不同:
//! - Sandbox 路径 (2.x): grouping = sandbox 自身 id。
//! - Task 路径 (1.6.33): container-Task 读 bundle 里的 sandbox-id annotation 当 grouping,
//!   落到 pod 那个已在跑的 shim 上; sandbox-Task 用自身 id。
//! 故 grouping 由调用方 (main.rs 按协议分支) 算好后传入。

use std::io::{Read, Write};
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};

use containerd_shim::{socket_address, Flags};

use crate::common::bundle::{load_bundle_spec, ContainerType};

/// 计算本次 `start` 的 grouping (决定 ttrpc socket, 同 grouping -> 同 socket -> 复用同一 shim)。
///
/// 统一规则 (同时覆盖两条协议, 见 ADR 决策4): 读 bundle `config.json` --
/// - **container-Task** (`container-type=container` 且带 `sandbox-id`) -> grouping = 该 `sandbox-id`,
///   落到 pod 那个已在跑的 shim 上。
/// - **sandbox-Task / 无 annotation / 读不到 config.json** (含 2.x Sandbox 路径) -> grouping = `flags.id`,
///   即「一个 shim 管一个 sandbox」, spawn 常驻 shim。
///
/// 2.x 路径 config.json 缺失或无 CRI annotation 时自然回落到 `flags.id`, 行为不变。
pub fn resolve_grouping(flags: &Flags) -> String {
    let bundle_dir = if flags.bundle.is_empty() {
        "." // containerd 把 shim 的 cwd 设为 bundle 目录。
    } else {
        flags.bundle.as_str()
    };

    match load_bundle_spec(bundle_dir) {
        Ok(spec) => match (spec.container_type, spec.sandbox_id) {
            (ContainerType::Container, Some(sid)) => sid,
            _ => flags.id.clone(),
        },
        // 读不到/解析失败: 回落 flags.id (不破坏 2.x 与裸 ctr run)。
        Err(_) => flags.id.clone(),
    }
}

/// 父进程 `start`: 给定 grouping, 计算 socket 地址、spawn 常驻 `run` 子进程,
/// 阻塞读子进程就绪信号后把地址打到 stdout。
///
/// `grouping` 决定 socket 地址 (同 grouping -> 同 socket -> 复用同一 shim 进程)。
pub fn action_start(flags: &Flags, grouping: &str) -> Result<(), Box<dyn std::error::Error>> {
    let address = socket_address(&flags.address, &flags.namespace, grouping);
    // socket 路径去掉 "unix://" 前缀用于文件系统检查/清理。
    let sock_path = address.strip_prefix("unix://").unwrap_or(&address);
    if let Some(parent) = std::path::Path::new(sock_path).parent() {
        std::fs::create_dir_all(parent)?;
    }
    let _ = std::fs::remove_file(sock_path);

    // spawn 常驻子进程: <bin> --namespace .. --id .. --address .. --socket <addr> run
    //
    // 握手 (对齐 containerd-shim 上游 spawn/serve): 子进程 stdout 用 pipe 接住, 父进程
    // **阻塞读到 EOF**--子进程只有在 socket 真正 bind 成功后才写地址并关闭 stdout (EOF)。
    // 这样父进程回给 containerd 地址时, socket 一定已就绪, 消除 `connect: no such file` 竞态。
    let exe = std::env::current_exe()?;
    let mut cmd = Command::new(exe);
    cmd.arg("--namespace")
        .arg(&flags.namespace)
        .arg("--id")
        .arg(&flags.id)
        .arg("--address")
        .arg(&flags.address)
        .arg("--socket")
        .arg(&address)
        .arg("run")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());

    // 透传 containerd 的 TTRPC_ADDRESS/GRPC_ADDRESS (事件发布用, P1 未用到但保留)。
    for key in ["TTRPC_ADDRESS", "GRPC_ADDRESS"] {
        if let Ok(v) = std::env::var(key) {
            cmd.env(key, v);
        }
    }

    unsafe {
        cmd.pre_exec(|| {
            let _ = nix::unistd::setsid();
            Ok(())
        });
    }

    let mut child = cmd.spawn()?;

    // 阻塞读子进程 stdout 到 EOF: 拿到就绪信号 (子进程写完地址后 redirect stdout>null 关闭 pipe)。
    let mut ready = String::new();
    if let Some(mut out) = child.stdout.take() {
        out.read_to_string(&mut ready)?;
    }

    let ready = ready.trim();
    let reported = if ready.is_empty() { address.as_str() } else { ready };

    // legacy 协议: stdout 打印 socket 地址 (containerd 读取并连接)。
    let mut stdout = std::io::stdout();
    stdout.write_all(reported.as_bytes())?;
    stdout.flush()?;

    Ok(())
}

/// 子进程就绪握手: socket 已 bind + serve 就绪后, 把地址写到 stdout、flush,
/// 再把 stdout 重定向到 /dev/null 关闭继承自父进程的 pipe (触发父进程 read EOF)。
pub fn signal_ready(socket_addr: &str) {
    let mut stdout = std::io::stdout();
    let _ = stdout.write_all(socket_addr.as_bytes());
    let _ = stdout.flush();

    // dup2(/dev/null, STDOUT) -- 关闭继承自父进程的 pipe 写端, 触发父进程 read EOF。
    if let Ok(devnull) = std::fs::OpenOptions::new().write(true).open("/dev/null") {
        use std::os::unix::io::AsRawFd;
        // STDOUT_FILENO = 1
        let _ = nix::unistd::dup2(devnull.as_raw_fd(), 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 写一个临时 bundle 目录含 config.json, 返回目录路径 (测试结束自动清理)。
    fn bundle_with(json: &str) -> tempdir_like::TempDir {
        let dir = tempdir_like::TempDir::new();
        std::fs::write(dir.path().join("config.json"), json).unwrap();
        dir
    }

    fn flags_with_bundle(bundle: &str, id: &str) -> Flags {
        Flags {
            bundle: bundle.to_string(),
            id: id.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn container_task_groups_onto_pod() {
        let dir = bundle_with(
            r#"{"annotations":{"io.kubernetes.cri.container-type":"container","io.kubernetes.cri.sandbox-id":"pod-xyz"}}"#,
        );
        let flags = flags_with_bundle(dir.path().to_str().unwrap(), "ctr-123");
        assert_eq!(resolve_grouping(&flags), "pod-xyz");
    }

    #[test]
    fn sandbox_task_uses_own_id() {
        let dir = bundle_with(
            r#"{"annotations":{"io.kubernetes.cri.container-type":"sandbox"}}"#,
        );
        let flags = flags_with_bundle(dir.path().to_str().unwrap(), "pod-xyz");
        assert_eq!(resolve_grouping(&flags), "pod-xyz");
    }

    #[test]
    fn missing_config_falls_back_to_id() {
        // 目录不存在的 config.json (模拟 2.x / 读不到) -> 回落 flags.id。
        let flags = flags_with_bundle("/nonexistent-bundle-dir-xyz", "sb-42");
        assert_eq!(resolve_grouping(&flags), "sb-42");
    }

    /// 极简 TempDir (避免引入 tempfile 依赖): 进程唯一目录, Drop 时删。
    mod tempdir_like {
        use std::path::{Path, PathBuf};
        use std::sync::atomic::{AtomicU32, Ordering};

        static COUNTER: AtomicU32 = AtomicU32::new(0);

        pub struct TempDir {
            path: PathBuf,
        }

        impl TempDir {
            pub fn new() -> Self {
                let n = COUNTER.fetch_add(1, Ordering::Relaxed);
                let path = std::env::temp_dir()
                    .join(format!("oas-shim-start-test-{}-{}", std::process::id(), n));
                std::fs::create_dir_all(&path).unwrap();
                Self { path }
            }

            pub fn path(&self) -> &Path {
                &self.path
            }
        }

        impl Drop for TempDir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.path);
            }
        }
    }
}