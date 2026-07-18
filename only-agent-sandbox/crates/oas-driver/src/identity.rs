//! 身份校验 + 进程管理（防 PID 复用）。
//!
//! 以 mount namespace inode 为锚点（jailer chroot 必建独立 mntns）。shim 在 create 完成
//! 时写 `$sandbox_dir/shim.meta`；re-attach / 应急杀前读候选 pid，`kill(pid,0)` +
//! `/proc/<pid>/ns/mnt` inode 比对 `shim.meta` 双校验。PID 复用的进程在另一 mntns，不命中。

use std::path::Path;

use nix::sys::signal::{kill, Signal};
use nix::sys::stat::stat;
use nix::unistd::Pid;
use serde::{Deserialize, Serialize};

/// shim 写入 `$sandbox_dir/shim.meta` 的权威身份记录。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShimMeta {
    /// firecracker 进程的 host pid。
    pub fc_pid: u32,
    /// create 时 `stat(/proc/<fc_pid>/ns/mnt).st_ino`。
    pub mntns_inode: u64,
    /// Unix 时间戳（秒）。
    pub started_at: i64,
    /// shim 自身 pid。
    pub shim_pid: u32,
}

impl ShimMeta {
    pub fn read(path: &Path) -> Option<Self> {
        let s = std::fs::read_to_string(path).ok()?;
        serde_json::from_str(&s).ok()
    }

    pub fn write(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let s = serde_json::to_vec(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        std::fs::write(path, s)
    }
}

/// `/proc/<pid>/ns/mnt` 的 inode。pid 不存在或无权限 → `Err`。
pub fn mntns_inode(pid: u32) -> std::io::Result<u64> {
    let p = format!("/proc/{pid}/ns/mnt");
    // /proc/pid/ns/mnt 是特殊符号链接；stat 跟随它返回命名空间的 inode。
    let s = stat(Path::new(&p)).map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    Ok(s.st_ino)
}

/// pid 是否存活（`kill(pid, 0)`）。
pub fn pid_alive(pid: u32) -> bool {
    matches!(kill(Pid::from_raw(pid as i32), None), Ok(()))
}

/// 候选 pid 是否就是 `meta` 记录的 firecracker：存活 + mntns inode 命中。
pub fn verify_fc_pid(pid: u32, meta: &ShimMeta) -> bool {
    if !pid_alive(pid) {
        return false;
    }
    match mntns_inode(pid) {
        Ok(ino) => ino == meta.mntns_inode,
        Err(_) => false,
    }
}

/// 杀进程：先 SIGTERM，短暂等待，再 SIGKILL。幂等（已死则 Ok）。
pub fn terminate_pid(pid: u32) {
    let p = Pid::from_raw(pid as i32);
    let _ = kill(p, Signal::SIGTERM);
    for _ in 0..20 {
        if !pid_alive(pid) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let _ = kill(p, Signal::SIGKILL);
}

/// 在 /proc 中找 firecracker 进程：comm == "firecracker" 且 `/proc/pid/root` 命中 jail root
/// （同 dev + inode）。`--new-pid-ns` 下也可靠的 host pid 获取法。shim re-attach 与 runtime
/// 应急杀共用。
pub fn find_fc_pid(jail_root: &Path) -> Option<u32> {
    let target = stat(jail_root).ok()?;
    for entry in std::fs::read_dir("/proc").ok()?.flatten() {
        let name = entry.file_name();
        let Some(pid) = name.to_str().and_then(|s| s.parse::<u32>().ok()) else {
            continue;
        };
        let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).unwrap_or_default();
        if !comm.starts_with("firecracker") {
            continue;
        }
        let root = match stat(Path::new(&format!("/proc/{pid}/root"))) {
            Ok(m) => m,
            Err(_) => continue,
        };
        if root.st_dev == target.st_dev && root.st_ino == target.st_ino {
            return Some(pid);
        }
    }
    None
}
