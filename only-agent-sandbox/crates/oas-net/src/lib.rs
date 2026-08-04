//! 编排层 → 网络层 契约（§2.2 / §3.4）。
//!
//! 为每 VM 建 netns + tap + IP / 路由 / NAT，确保 snapshot 克隆的网络可确定性重建。
//! 所有进 netns 的 syscall 关进 `spawn_blocking` 专用线程（RAII guard 还原），见 §3.4。

/// 网络上下文：建好后返回，供 snapshot 克隆确定性重建，`teardown` 据此幂等回收。
#[derive(Debug, Clone)]
pub struct NetConfig {
    /// `/var/run/netns/oas-<sandbox_id>`。
    pub netns_path: String,
    /// 克隆间保持一致，让 guest 无感。
    pub tap_name: String,
    pub mac: String,
    pub pod_ip: String,
    pub gateway: String,
    pub lease: oas_types::IpLease,
    /// 主机侧 veth 名（A1 PodIP 附着）；空串表示未启用。
    pub host_veth: String,
}

/// 网络层错误。
#[derive(Debug, thiserror::Error)]
pub enum NetError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("ipam exhausted: {0}")]
    IpamExhausted(String),
    #[error("{0}")]
    Other(String),
}

/// 编排层 → 网络层接口（§2.2）。
///
/// `setup` 建 netns + tap + 配 IP/路由/NAT，返回网络上下文；
/// `teardown` 幂等回收 netns / tap / 释放 IP 租约。
#[async_trait::async_trait]
pub trait NetworkManager: Send + Sync {
    async fn setup(&self, sandbox_id: &str) -> Result<NetConfig, NetError>;

    async fn teardown(&self, net: &NetConfig) -> Result<(), NetError>;
}

pub mod cni;
pub mod forward;
pub mod real;
pub use cni::{host_veth_plan, host_veth_setup_cmds, host_veth_teardown_cmds, HostVethPlan};
pub use forward::{
    guest_egress_cmds, guest_port_forward_cmds, host_egress_cmds, GuestEgress, GuestPortForward,
    HostEgress,
};
pub use real::NetManager;
