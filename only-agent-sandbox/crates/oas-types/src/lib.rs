//! 跨 crate 共享领域模型（§4 `oas-types`）。
//!
//! 无重依赖，只靠 serde。`SandboxRecord` / `ContainerRecord` 等为持久化层
//! (`Store` trait) 与编排层共享的事实源载体，字段原样透传（§6 红线 1）。

use std::collections::HashMap;

// ---------------------------------------------------------------------------
// IPAM 租约（net 与 store 共用）
// ---------------------------------------------------------------------------

/// IPAM 分配的 IP 租约。崩溃后不可重复分配（§3.6 `ipam` 表）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct IpLease {
    /// 分配到的 IP，如 `10.244.1.5`。
    pub ip: String,
    /// 所属 CIDR，如 `10.244.1.0/24`。
    pub cidr: String,
    /// 持有该租约的 sandbox。
    pub sandbox_id: String,
}

// ---------------------------------------------------------------------------
// 状态枚举（映射 CRI proto state）
// ---------------------------------------------------------------------------

/// 映射 `PodSandboxStatus.state`（§3.2 Sandbox FSM）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SandboxState {
    Ready,
    NotReady,
}

/// 映射 `ContainerStatus.state`（§3.2 Container FSM）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ContainerState {
    Created,
    Running,
    Exited,
    Unknown,
}

/// 带外 agent 回灌的进程退出 reason（§21.4）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ContainerExitReason {
    Completed,
    Error,
    OomKilled,
}

// ---------------------------------------------------------------------------
// 原样透传的 CRI metadata / 资源（轻量自有类型，不引 proto）
// ---------------------------------------------------------------------------

/// `PodSandboxMetadata`，原样存取（§6 红线 1）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SandboxMetadata {
    pub name: String,
    pub namespace: String,
    /// Pod UID。
    pub uid: String,
    pub attempt: u32,
}

/// `ContainerMetadata`，原样存取。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ContainerMetadata {
    pub name: String,
    pub attempt: u32,
}

/// 挂载传播模式（对应 CRI `MountPropagation`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum MountPropagation {
    Private,
    HostToContainer,
    Bidirectional,
}

/// 挂载项（轻量自有类型）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Mount {
    pub container_path: String,
    pub host_path: String,
    pub read_only: bool,
    pub propagation: MountPropagation,
}

/// Linux 资源限制（轻量，供 §9 type 表一致性校验）。
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct LinuxResources {
    pub cpu_shares: Option<u64>,
    pub memory_limit_bytes: Option<i64>,
}

/// 环境变量键值对（对应 CRI `KeyValue`）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct KeyValue {
    pub key: String,
    pub value: String,
}

/// DNS 配置（对应 CRI `DNSConfig`，§13 注入 guest `/etc/resolv.conf`）。
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct DnsConfig {
    pub servers: Vec<String>,
    pub searches: Vec<String>,
    pub options: Vec<String>,
}

// ---------------------------------------------------------------------------
// Records（持久化层事实源，§3.6 五张表中的 sandbox / container）
// ---------------------------------------------------------------------------

/// `sandbox` 表记录。`pod_uid` 为 ★幂等键；metadata/labels/annotations 原样。
///
/// 注意：`vm_id` 用 `u64` 而非 `oas_driver::VmId`，避免 `oas-types` 反向依赖
/// `oas-driver`。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SandboxRecord {
    pub sandbox_id: String,
    /// ★幂等键（kubelet 重试不产生重复 VM）。
    pub pod_uid: String,
    pub metadata: SandboxMetadata,
    pub labels: HashMap<String, String>,
    pub annotations: HashMap<String, String>,
    pub type_id: u8,
    pub vm_id: u64,
    pub netns_path: String,
    pub tap_name: String,
    pub mac: String,
    pub pod_ip: String,
    pub gateway: String,
    pub rw_layer_path: Option<String>,
    pub cloud_disk_dev: Option<String>,
    pub state: SandboxState,
    /// Unix 时间戳（秒）。
    pub created_at: i64,
}

/// `container` 表记录。`exit_code` / `reason` / `message` 由带外 agent 回灌（§21.4）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ContainerRecord {
    pub container_id: String,
    pub sandbox_id: String,
    pub metadata: ContainerMetadata,
    /// 工作镜像引用，须命中该 type 的 `image_whitelist`（§9）。
    pub image: String,
    pub command: Vec<String>,
    pub args: Vec<String>,
    /// `KEY=VAL` 形式。
    pub env: Vec<String>,
    pub mounts: Vec<Mount>,
    pub resources: LinuxResources,
    pub state: ContainerState,
    pub created_at: i64,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
    /// 进程退出码（带外 agent 回灌；收不到则 state=UNKNOWN，严禁反推 0）。
    pub exit_code: i32,
    pub reason: Option<ContainerExitReason>,
    pub message: String,
    pub labels: HashMap<String, String>,
    pub annotations: HashMap<String, String>,
}

// ---------------------------------------------------------------------------
// Filters（List* 收敛，§21.3）
// ---------------------------------------------------------------------------

/// `list_sandboxes` 过滤器：按 id / pod_uid / label / state 收敛。
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct SandboxFilter {
    pub id: Option<String>,
    pub pod_uid: Option<String>,
    pub label_selector: HashMap<String, String>,
    pub state: Option<SandboxState>,
}

/// `list_containers` 过滤器：按 id / sandbox_id / label / state 收敛。
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct ContainerFilter {
    pub id: Option<String>,
    pub sandbox_id: Option<String>,
    pub label_selector: HashMap<String, String>,
    pub state: Option<ContainerState>,
}
