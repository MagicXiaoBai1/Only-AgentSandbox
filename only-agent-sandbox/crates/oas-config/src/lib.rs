//! 全局配置（§配置加载）。
//!
//! MVP 阶段 `Default` 给硬编码值即可跑；`Config::load(path)` 是文件读取的扩展性接缝——
//! TOML 文件存在则读、否则回落 `Default`。runtime `--config <path>` 透传给 shim，两边读
//! 同一份单一事实源。
//!
//! 故意不含实现细节（如何 spawn jailer / 如何调 firecracker API），只持有路径、uid/gid、
//! 类型表等静态配置。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// 网络配置（MVP：固定 tap0 + 静态 IP）
// ---------------------------------------------------------------------------

/// MVP 网络配置：固定 tap 名 + 静态地址（全沙箱相同，靠 netns 区分）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetConfig {
    /// jail root 内 / netns 内的固定 tap 名（bake 时 `host_dev_name` 必须一致）。
    pub tap_name: String,
    /// tap 设备的网关 IP（不带前缀），如 `172.16.0.1`。
    pub tap_gateway: String,
    /// tap 网段前缀长度，如 `30`。
    pub tap_prefix: u8,
    /// guest eth0 静态 IP（不带前缀），如 `172.16.0.2`。镜像侧须已配好。
    pub guest_ip: String,
    /// bake 时 `network-interfaces` 的 `guest_mac`，全沙箱固定。
    pub guest_mac: String,
    /// CRI pod IPAM CIDR（kubelet 视角的 pod IP 池，与 eth0 实际 IP 解耦）。
    pub pod_cidr: String,
    /// pod 网关（信息字段）。
    pub pod_gateway: String,
    /// guest-agent 监听端口；A1 要求 `PodIP:port` DNAT 到 `guest_ip:port`。
    pub guest_agent_port: u16,
    /// 是否在 host 建 veth 并把 PodIP 挂到 netns eth0（同节点可达）。
    #[serde(default = "default_true")]
    pub enable_host_veth: bool,
    /// netns 内承载 PodIP 的接口名。
    #[serde(default = "default_pod_iface")]
    pub pod_iface: String,
    /// 是否为 guest 出向安装 NAT/转发规则。
    #[serde(default = "default_true")]
    pub enable_guest_egress: bool,
    /// shim Create 是否等待 guest-agent TCP 可达。
    #[serde(default = "default_true")]
    pub wait_guest_agent: bool,
    /// 等待 guest-agent 的超时（秒）。
    #[serde(default = "default_agent_wait_secs")]
    pub guest_agent_wait_secs: u64,
}

fn default_true() -> bool {
    true
}

fn default_pod_iface() -> String {
    "eth0".into()
}

fn default_agent_wait_secs() -> u64 {
    30
}

// ---------------------------------------------------------------------------
// Sandbox 类型表
// ---------------------------------------------------------------------------

/// 单个 sandbox 类型。`bundle` 指向 `$artifacts_dir/snapshots/<bundle>/` 自包含模板。
/// `vcpu`/`mem_mib` 仅用于 manager 资源预算校验——snapshot 已 bake vcpu/mem，restore 不设。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxType {
    pub type_id: u8,
    /// `snapshots/` 下目录名，如 `base-0`。
    pub bundle: String,
    pub vcpu: u32,
    pub mem_mib: u32,
    pub has_rw_layer: bool,
    pub has_cloud_disk: bool,
    pub image_whitelist: Vec<String>,
}

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

/// 全局配置。runtime 与 shim 共享（同一 `--config` 路径）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub firecracker_bin: PathBuf,
    pub jailer_bin: PathBuf,
    /// jailer `--chroot-base-dir`。jail root = `$chroot_base_dir/firecracker/<sid>/root`。
    pub chroot_base_dir: PathBuf,
    pub jailer_uid: u32,
    pub jailer_gid: u32,
    /// 产物根：下含 `snapshots/<bundle>/`。
    pub artifacts_dir: PathBuf,
    /// shim socket + netns 目录。
    pub run_base_dir: PathBuf,
    /// shim / firecracker 日志目录。
    pub log_dir: PathBuf,
    /// redb 持久化路径。
    pub store_path: PathBuf,
    /// CRI UDS。
    pub cri_socket: PathBuf,
    /// rw 层 ext4 存放目录（`<sid>.ext4`）。
    pub rw_base_dir: PathBuf,
    /// rw 层默认大小（MiB）。
    pub rw_size_mib: u64,

    pub net: NetConfig,
    pub types: Vec<SandboxType>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            firecracker_bin: PathBuf::from("/home/yunfei/workspace/snap_double_shot/bin/firecracker"),
            jailer_bin: PathBuf::from("/home/yunfei/workspace/snap_double_shot/bin/jailer"),
            chroot_base_dir: PathBuf::from("/home/yunfei/oas-test"),
            jailer_uid: 1234,
            jailer_gid: 1234,
            artifacts_dir: PathBuf::from("/var/lib/oas/artifacts"),
            run_base_dir: PathBuf::from("/run/oas"),
            log_dir: PathBuf::from("/var/log/oas"),
            store_path: PathBuf::from("/var/lib/oas/store.redb"),
            cri_socket: PathBuf::from("/run/oas.sock"),
            rw_base_dir: PathBuf::from("/var/lib/oas/rw"),
            rw_size_mib: 1024,
            net: NetConfig {
                tap_name: "tapH0".into(),
                tap_gateway: "172.16.0.1".into(),
                tap_prefix: 30,
                guest_ip: "172.16.0.2".into(),
                guest_mac: "06:00:AC:10:00:02".into(),
                pod_cidr: "10.244.0.0/24".into(),
                pod_gateway: "10.244.0.254".into(),
                guest_agent_port: 10000,
                enable_host_veth: true,
                pod_iface: "eth0".into(),
                enable_guest_egress: true,
                wait_guest_agent: true,
                guest_agent_wait_secs: 30,
            },
            types: vec![
                SandboxType {
                    type_id: 0,
                    bundle: "base-0".into(),
                    vcpu: 1,
                    mem_mib: 512,
                    has_rw_layer: false,
                    has_cloud_disk: false,
                    image_whitelist: vec!["img-a".into(), "guest-agent".into()],
                },
                SandboxType {
                    type_id: 1,
                    bundle: "base-1".into(),
                    vcpu: 2,
                    mem_mib: 1024,
                    has_rw_layer: true,
                    has_cloud_disk: false,
                    image_whitelist: vec!["img-b".into(), "guest-agent".into()],
                },
                SandboxType {
                    type_id: 2,
                    bundle: "base-2".into(),
                    vcpu: 4,
                    mem_mib: 2048,
                    has_rw_layer: true,
                    has_cloud_disk: true,
                    image_whitelist: vec!["img-c".into(), "guest-agent".into()],
                },
            ],
        }
    }
}

impl Config {
    /// 从 TOML 文件加载；文件不存在或解析失败 → 回落 `Default`（MVP 容错）。
    pub fn load(path: &Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(s) => toml::from_str(&s).unwrap_or_else(|_| Self::default()),
            Err(_) => Self::default(),
        }
    }

    /// `$artifacts_dir/snapshots/<bundle>/`。
    pub fn bundle_dir(&self, bundle: &str) -> PathBuf {
        self.artifacts_dir.join("snapshots").join(bundle)
    }

    /// `$chroot_base_dir/firecracker/<sid>/root`（jail root = firecracker 的 `/`）。
    pub fn jail_root(&self, sandbox_id: &str) -> PathBuf {
        self.chroot_base_dir
            .join("firecracker")
            .join(sandbox_id)
            .join("root")
    }

    /// `$chroot_base_dir/firecracker/<sid>/`（shim.meta、jail root 的父目录）。
    pub fn sandbox_dir(&self, sandbox_id: &str) -> PathBuf {
        self.chroot_base_dir
            .join("firecracker")
            .join(sandbox_id)
    }

    /// `$run_base_dir/oas-shim-<sid>.sock`（ttrpc socket）。
    pub fn shim_socket(&self, sandbox_id: &str) -> PathBuf {
        self.run_base_dir
            .join(format!("oas-shim-{sandbox_id}.sock"))
    }

    /// `/var/run/netns/oas-<sid>`（`ip netns add` 标准位置，供 jailer `--netns`）。
    pub fn netns_path(&self, sandbox_id: &str) -> PathBuf {
        PathBuf::from(format!("/var/run/netns/oas-{sandbox_id}"))
    }

    /// netns 名（`ip netns add/del` 参数）。
    pub fn netns_name(&self, sandbox_id: &str) -> String {
        format!("oas-{sandbox_id}")
    }

    /// `$log_dir/oas-shim-<sid>.log`。
    pub fn shim_log(&self, sandbox_id: &str) -> PathBuf {
        self.log_dir.join(format!("oas-shim-{sandbox_id}.log"))
    }

    /// 按 `type_id` 取类型；越界返回 `None`。
    pub fn get_type(&self, type_id: u8) -> Option<&SandboxType> {
        self.types.iter().find(|t| t.type_id == type_id)
    }

    /// 全部白名单镜像（供 `list_images`）；按出现顺序去重。
    pub fn all_images(&self) -> Vec<&str> {
        let mut out: Vec<&str> = Vec::new();
        for t in &self.types {
            for img in &t.image_whitelist {
                let base = image_base_name(img);
                if !out.iter().any(|x| image_base_name(x) == base) {
                    out.push(img.as_str());
                }
            }
        }
        out
    }

    /// 镜像是否被任一 type 白名单允许（按 base name 匹配）。
    pub fn image_allowed_any(&self, image: &str) -> bool {
        self.types.iter().any(|t| t.image_allowed(image))
    }
}

/// 取镜像引用的 base name：去 `sha256:`/`sha512:` 前缀、去 registry/path、去 tag。
/// 与原 `types_table::image_base_name` 行为一致（迁过来）。
pub fn image_base_name(image: &str) -> &str {
    let image = image
        .strip_prefix("sha256:")
        .or_else(|| image.strip_prefix("sha512:"))
        .unwrap_or(image);
    let after_slash = image.rsplit('/').next().unwrap_or(image);
    match after_slash.rfind(':') {
        Some(i) => &after_slash[..i],
        None => after_slash,
    }
}

impl SandboxType {
    /// 镜像是否在本 type 白名单内（按 base name 匹配）。
    pub fn image_allowed(&self, image: &str) -> bool {
        let base = image_base_name(image);
        self.image_whitelist
            .iter()
            .any(|w| image_base_name(w) == base)
    }
}
