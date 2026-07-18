//! 真 `NetworkManager`：每沙箱 netns + 固定名 tap0 + 网关 IP。
//!
//! 所有 `ip` 调用走 `spawn_blocking`（design doc §3.4：netns syscall 关专用线程）。
//! tap 直接在 netns 内创建（`ip netns exec <ns> ip tuntap add`），避免 host 侧 tap0 同名竞态。
//! IPAM：pod_ip 从 store `lease_ip(pod_cidr)`；tap 网关 IP 固定（cfg.net）。MVP 两者解耦。

use std::process::Command;
use std::sync::Arc;

use async_trait::async_trait;
use oas_config::Config;
use oas_store::Store;
use oas_types::IpLease;

use crate::{NetConfig, NetError, NetworkManager};

/// 真 netns + tap 实现。
pub struct NetManager {
    store: Arc<dyn Store>,
    cfg: Arc<Config>,
}

impl NetManager {
    pub fn new(store: Arc<dyn Store>, cfg: Arc<Config>) -> Self {
        Self { store, cfg }
    }
}

fn run_ip(args: &[&str]) -> Result<(), NetError> {
    let out = Command::new("ip").args(args).output().map_err(|e| {
        NetError::Other(format!("ip {}: {e}", args.join(" ")))
    })?;
    if !out.status.success() {
        return Err(NetError::Other(format!(
            "ip {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr)
        )));
    }
    Ok(())
}

#[async_trait]
impl NetworkManager for NetManager {
    async fn setup(&self, sandbox_id: &str) -> Result<NetConfig, NetError> {
        let ns = self.cfg.netns_name(sandbox_id);
        let netns_path = self.cfg.netns_path(sandbox_id);
        let tap = self.cfg.net.tap_name.clone();
        let gateway = self.cfg.net.tap_gateway.clone();
        let prefix = self.cfg.net.tap_prefix;
        let mac = self.cfg.net.guest_mac.clone();
        let cidr = self.cfg.net.pod_cidr.clone();
        let gw_pod = self.cfg.net.pod_gateway.clone();
        let store = self.store.clone();

        tokio::task::spawn_blocking(move || {
            // IPAM 租约（崩溃后不重复分配）。
            let lease = store
                .lease_ip(&cidr)
                .map_err(|e| NetError::Other(e.to_string()))?;
            let mut lease = lease;
            lease.sandbox_id = ns.clone();

            // 建 netns + 在其内建 tap0 + 配网关 IP + up。
            // netns add 幂等（已存在则忽略错误）。
            let _ = run_ip(&["netns", "add", &ns]);
            run_ip(&["netns", "exec", &ns, "ip", "tuntap", "add", "dev", &tap, "mode", "tap"])?;
            run_ip(&[
                "netns", "exec", &ns, "ip", "addr", "add",
                &format!("{gateway}/{prefix}"),
                "dev", &tap,
            ])?;
            run_ip(&["netns", "exec", &ns, "ip", "link", "set", &tap, "up"])?;

            Ok(NetConfig {
                netns_path: netns_path.to_string_lossy().into_owned(),
                tap_name: tap,
                mac,
                pod_ip: lease.ip.clone(),
                gateway: gw_pod,
                lease,
            })
        })
        .await
        .map_err(|e| NetError::Other(format!("blocking join: {e}")))?
    }

    async fn teardown(&self, net: &NetConfig) -> Result<(), NetError> {
        // netns_path = /var/run/netns/oas-<sid>；name = basename。
        let ns = net
            .netns_path
            .rsplit('/')
            .next()
            .unwrap_or(&net.netns_path)
            .to_string();
        let lease = net.lease.clone();
        let store = self.store.clone();
        tokio::task::spawn_blocking(move || {
            // 删 netns 会一并清掉其内的 tap。幂等（不存在则忽略）。
            let _ = run_ip(&["netns", "del", &ns]);
            store
                .release_ip(&lease)
                .map_err(|e| NetError::Other(e.to_string()))?;
            Ok(())
        })
        .await
        .map_err(|e| NetError::Other(format!("blocking join: {e}")))?
    }
}

// 静默未用警告（IpLease 仅用于类型推断）
#[allow(dead_code)]
fn _ensure_lease_used() -> Option<IpLease> {
    None
}
