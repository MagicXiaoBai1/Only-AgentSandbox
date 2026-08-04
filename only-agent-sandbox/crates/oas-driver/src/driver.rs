//! Runtime 侧 `RealDriver`：ttrpc client + spawn shim（setsid）+ 再发现 + 应急杀。
//!
//! trait 方法是 async，ttrpc sync client 调用经 `spawn_blocking` 包裹。索引
//! `sandbox_id → socket 路径` 在 `new()` 时扫 `$run_base/oas-shim-*.sock` 重建。

use std::collections::HashMap;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use oas_config::Config;
use oas_types::SandboxId;

use crate::generated::shim::{CreateRequest, StateRequest, StopRequest};
use crate::generated::shim_ttrpc::ShimClient;
use crate::identity::{find_fc_pid, terminate_pid, ShimMeta};
use crate::{DriverError, VmLifecycle, VmNet, VmSpec, VmStatus};

/// runtime 侧 FirecrackerDriver 真 impl。
pub struct RealDriver {
    cfg: Arc<Config>,
    cfg_path: PathBuf,
    index: Arc<Mutex<HashMap<String, PathBuf>>>,
}

impl RealDriver {
    /// 加载配置（`cfg_path` 同时透传给 spawn 的 shim）+ 扫 socket 目录重建索引。
    pub fn new(cfg_path: PathBuf) -> Self {
        let cfg = Arc::new(Config::load(&cfg_path));
        let index = Arc::new(Mutex::new(HashMap::new()));
        let me = Self {
            cfg,
            cfg_path,
            index,
        };
        me.rediscover();
        me
    }

    /// 扫 `$run_base/oas-shim-*.sock`：连得上 → 入索引；stale → unlink。
    fn rediscover(&self) {
        let dir = self.cfg.run_base_dir.clone();
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return;
        };
        let mut idx = self.index.lock().unwrap();
        for e in entries.flatten() {
            let name = e.file_name();
            let Some(s) = name.to_str() else { continue };
            let Some(sid) = s.strip_prefix("oas-shim-").and_then(|r| r.strip_suffix(".sock")) else {
                continue;
            };
            let sock = e.path();
            if try_state(&sock).is_ok() {
                idx.insert(sid.to_string(), sock);
            } else {
                let _ = std::fs::remove_file(&sock);
            }
        }
    }

    /// spawn shim 进程（setsid 脱离 runtime session）。同步，须在 spawn_blocking 内调。
    fn spawn_shim(&self, sid: &str) -> Result<(), DriverError> {
        let exe = std::env::current_exe().map_err(|e| DriverError::Other(format!("current_exe: {e}")))?;
        let socket = self.cfg.shim_socket(sid);
        let log = self.cfg.shim_log(sid);
        if let Some(p) = log.parent() {
            std::fs::create_dir_all(p)?;
        }
        let log_file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log)?;
        let mut cmd = Command::new(exe);
        cmd.arg("shim")
            .arg("--config")
            .arg(&self.cfg_path)
            .arg("--sandbox-id")
            .arg(sid)
            .arg("--socket")
            .arg(&socket)
            .arg("--log-file")
            .arg(&log)
            .stdin(Stdio::null())
            .stdout(Stdio::from(log_file.try_clone()?))
            .stderr(Stdio::from(log_file));
        unsafe {
            cmd.pre_exec(|| {
                let _ = nix::unistd::setsid();
                Ok(())
            });
        }
        cmd.spawn().map_err(|e| DriverError::Shim(format!("spawn shim: {e}")))?;
        Ok(())
    }
}

#[async_trait::async_trait]
impl crate::FirecrackerDriver for RealDriver {
    async fn create_vm(
        &self,
        id: &SandboxId,
        net: &VmNet,
        type_id: u8,
        spec: VmSpec,
    ) -> Result<(), DriverError> {
        let sid = id.as_str().to_string();
        let ty = self
            .cfg
            .get_type(type_id)
            .ok_or_else(|| DriverError::Other(format!("unknown type_id {type_id}")))?;
        if ty.has_cloud_disk {
            return Err(DriverError::Unsupported(format!(
                "type {type_id} cloud-disk restore not supported in MVP"
            )));
        }
        let bundle_dir = self.cfg.bundle_dir(&ty.bundle);
        let socket = self.cfg.shim_socket(&sid);
        let cfg = self.cfg.clone();
        let cfg_path = self.cfg_path.clone();
        let index = self.index.clone();
        let net = net.clone();
        let rw = spec.rw_layer_path.clone().unwrap_or_default();
        let cd = spec.cloud_disk_dev.clone().unwrap_or_default();

        tokio::task::spawn_blocking(move || {
            // 确保 shim 在（socket 不在则 spawn + 等）。
            if !socket.exists() {
                spawn_and_wait(&cfg_path, &cfg, &sid, &socket)?;
            }
            let req = CreateRequest {
                sandbox_id: sid.clone(),
                bundle_dir: bundle_dir.to_string_lossy().into_owned(),
                netns_path: net.netns_path,
                tap_name: net.tap_name,
                rw_layer_path: rw,
                cloud_disk_dev: cd,
                jailer_uid: cfg.jailer_uid,
                jailer_gid: cfg.jailer_gid,
                chroot_base_dir: cfg.chroot_base_dir.to_string_lossy().into_owned(),
                firecracker_bin: cfg.firecracker_bin.to_string_lossy().into_owned(),
                jailer_bin: cfg.jailer_bin.to_string_lossy().into_owned(),
                ..Default::default()
            };
            let client = connect(&socket)?;
            let sc = ShimClient::new(client);
            let resp = sc
                .create(ttrpc::context::Context::default(), &req)
                .map_err(|e| DriverError::Shim(e.to_string()))?;
            if resp.state == "Running" {
                index.lock().unwrap().insert(sid, socket);
                Ok(())
            } else {
                Err(DriverError::Shim(format!("create Failed: {}", resp.error)))
            }
        })
        .await
        .map_err(|e| DriverError::Shim(format!("blocking join: {e}")))?
    }

    async fn get_vm(&self, id: &SandboxId) -> Result<VmStatus, DriverError> {
        let sid = id.as_str().to_string();
        let cfg_path = self.cfg_path.clone();
        let cfg = self.cfg.clone();
        let socket = self.cfg.shim_socket(&sid);
        let id2 = id.clone();
        tokio::task::spawn_blocking(move || {
            // 有界重试：连不上 → re-spawn shim（re-attach）→ 再 State。
            let mut last = None;
            for _ in 0..3 {
                match try_state(&socket) {
                    Ok(st) => return Ok(state_to_status(&id2, &st)),
                    Err(e) => last = Some(e),
                }
                // socket 不在或连不上 → re-spawn。
                let _ = spawn_and_wait(&cfg_path, &cfg, &sid, &socket);
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(last.unwrap_or_else(|| DriverError::NotFound(sid)))
        })
        .await
        .map_err(|e| DriverError::Shim(format!("blocking join: {e}")))?
    }

    async fn delete_vm(&self, id: &SandboxId) -> Result<(), DriverError> {
        let sid = id.as_str().to_string();
        let cfg = self.cfg.clone();
        let socket = self.cfg.shim_socket(&sid);
        let sandbox_dir = cfg.sandbox_dir(&sid);
        let jail_root = cfg.jail_root(&sid);
        let index = self.index.clone();
        tokio::task::spawn_blocking(move || {
            // 先试 shim ttrpc Stop。
            match connect(&socket) {
                Ok(client) => {
                    let sc = ShimClient::new(client);
                    let _ = sc.stop(ttrpc::context::Context::default(), &StopRequest {
                        sandbox_id: sid.clone(),
                        ..Default::default()
                    });
                }
                Err(_) => emergency_kill(&sandbox_dir, &jail_root, &socket)?,
            }
            // 给 shim 退出留时间，再兜底应急杀（防 shim 没清干净）。
            std::thread::sleep(Duration::from_millis(200));
            let _ = emergency_kill(&sandbox_dir, &jail_root, &socket);
            index.lock().unwrap().remove(&sid);
            Ok(())
        })
        .await
        .map_err(|e| DriverError::Shim(format!("blocking join: {e}")))?
    }

    async fn list_vm(&self) -> Result<Vec<VmStatus>, DriverError> {
        let entries: Vec<(String, PathBuf)> = {
            let idx = self.index.lock().unwrap();
            idx.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
        };
        let mut out = Vec::new();
        for (sid, sock) in entries {
            let id = SandboxId(sid.clone());
            match try_state(&sock) {
                Ok(st) => out.push(state_to_status(&id, &st)),
                Err(_) => {
                    out.push(VmStatus {
                        id,
                        started: false,
                        healthy: false,
                        lifecycle: VmLifecycle::Degraded,
                    });
                }
            }
        }
        Ok(out)
    }

    // --- 容器原语 MVP no-op ---

    async fn create_container(
        &self,
        _vm_id: &SandboxId,
        _container_id: &str,
        _spec: crate::ContainerSpec,
    ) -> Result<(), DriverError> {
        Ok(())
    }

    async fn start_container(&self, _vm_id: &SandboxId, _container_id: &str) -> Result<(), DriverError> {
        Ok(())
    }

    async fn stop_container(
        &self,
        _vm_id: &SandboxId,
        _container_id: &str,
        _timeout_sec: i64,
    ) -> Result<(), DriverError> {
        Ok(())
    }

    async fn remove_container(&self, _vm_id: &SandboxId, _container_id: &str) -> Result<(), DriverError> {
        Ok(())
    }

    async fn get_container_status(
        &self,
        _vm_id: &SandboxId,
        _container_id: &str,
    ) -> Result<crate::ContainerRuntimeStatus, DriverError> {
        Ok(crate::ContainerRuntimeStatus::default())
    }
}

// ---- 自由函数（spawn_blocking 内用，不持 &self） -------------------------

fn connect(socket: &Path) -> Result<ttrpc::Client, DriverError> {
    let addr = format!("unix://{}", socket.display());
    ttrpc::Client::connect(&addr).map_err(|e| DriverError::Shim(format!("connect {}: {e}", socket.display())))
}

fn try_state(socket: &Path) -> Result<String, DriverError> {
    let client = connect(socket)?;
    let sc = ShimClient::new(client);
    let resp = sc
        .state(
            ttrpc::context::Context::default(),
            &StateRequest::default(),
        )
        .map_err(|e| DriverError::Shim(e.to_string()))?;
    Ok(resp.state)
}

fn spawn_and_wait(
    cfg_path: &Path,
    cfg: &Config,
    sid: &str,
    socket: &Path,
) -> Result<(), DriverError> {
    let exe =
        std::env::current_exe().map_err(|e| DriverError::Other(format!("current_exe: {e}")))?;
    let log = cfg.shim_log(sid);
    if let Some(p) = log.parent() {
        std::fs::create_dir_all(p)?;
    }
    let log_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log)?;
    let mut cmd = Command::new(exe);
    cmd.arg("shim")
        .arg("--config")
        .arg(cfg_path)
        .arg("--sandbox-id")
        .arg(sid)
        .arg("--socket")
        .arg(socket)
        .arg("--log-file")
        .arg(&log)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log_file.try_clone()?))
        .stderr(Stdio::from(log_file));
    unsafe {
        cmd.pre_exec(|| {
            let _ = nix::unistd::setsid();
            Ok(())
        });
    }
    cmd.spawn().map_err(|e| DriverError::Shim(format!("spawn shim: {e}")))?;
    // 等 socket。
    let mut waited = 0;
    while !socket.exists() {
        std::thread::sleep(Duration::from_millis(50));
        waited += 1;
        if waited > 200 {
            return Err(DriverError::Shim("timeout waiting shim socket".into()));
        }
    }
    Ok(())
}

/// shim 不可达时的应急杀：读 shim.meta → 身份校验 → 杀 firecracker → 清 jail root/socket。
///
/// 公开供 `oas-ctrd-shim` 的 `delete` action（containerd 在 shim 不可达时调二进制做回收）
/// 复用——Path A 与 Path B 共用同一套确定性资源释放（见 ADR 0009）。
pub fn emergency_kill(sandbox_dir: &Path, jail_root: &Path, socket: &Path) -> Result<(), DriverError> {
    let meta_path = sandbox_dir.join("shim.meta");
    if let Some(meta) = ShimMeta::read(&meta_path) {
        if crate::identity::verify_fc_pid(meta.fc_pid, &meta) {
            terminate_pid(meta.fc_pid);
        }
    } else if let Some(pid) = find_fc_pid(jail_root) {
        // 无 meta 但有孤儿 firecracker 命中 jail root → 杀。
        terminate_pid(pid);
    }
    // 调试开关：保留 jail root 现场（OAS_DEBUG_KEEP_JAIL=1）便于排查 firecracker 恢复失败。
    if std::env::var("OAS_DEBUG_KEEP_JAIL").as_deref() != Ok("1") {
        let _ = std::fs::remove_dir_all(jail_root);
    }
    let _ = std::fs::remove_file(&meta_path);
    let _ = std::fs::remove_file(socket);
    Ok(())
}

fn state_to_status(id: &SandboxId, state: &str) -> VmStatus {
    let lifecycle = match state {
        "Running" => VmLifecycle::Running,
        "Stopped" => VmLifecycle::Stopped,
        "Failed" => VmLifecycle::Degraded,
        _ => VmLifecycle::Creating,
    };
    VmStatus {
        id: id.clone(),
        started: lifecycle == VmLifecycle::Running,
        healthy: lifecycle == VmLifecycle::Running,
        lifecycle,
    }
}
