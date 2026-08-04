//! 真 `NetworkManager`：每沙箱 netns + tap +（可选）host veth 挂 PodIP + guest 出向 NAT。

use std::process::Command;
use std::sync::Arc;

use async_trait::async_trait;
use oas_config::Config;
use oas_store::Store;
use oas_types::IpLease;

use crate::cni::{host_veth_plan, host_veth_setup_cmds, host_veth_teardown_cmds};
use crate::forward::{
    guest_egress_cmds, guest_port_forward_cmds, host_egress_cmds, GuestEgress, GuestPortForward,
    HostEgress,
};
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

fn run_ip(args: &[String]) -> Result<(), NetError> {
    let strs: Vec<&str> = args.iter().map(String::as_str).collect();
    let out = Command::new("ip").args(&strs).output().map_err(|e| {
        NetError::Other(format!("ip {}: {e}", strs.join(" ")))
    })?;
    if !out.status.success() {
        return Err(NetError::Other(format!(
            "ip {} failed: {}",
            strs.join(" "),
            String::from_utf8_lossy(&out.stderr)
        )));
    }
    Ok(())
}

fn run_ip_str(args: &[&str]) -> Result<(), NetError> {
    run_ip(&args.iter().map(|s| (*s).to_string()).collect::<Vec<_>>())
}

fn run_iptables(args: &[String]) -> Result<(), NetError> {
    let out = Command::new("iptables")
        .args(args)
        .output()
        .map_err(|e| NetError::Other(format!("iptables: {e}")))?;
    if !out.status.success() {
        return Err(NetError::Other(format!(
            "iptables {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        )));
    }
    Ok(())
}

fn run_iptables_in_netns(ns: &str, args: &[String]) -> Result<(), NetError> {
    let mut cmd = Command::new("ip");
    cmd.args(["netns", "exec", ns, "iptables"]);
    cmd.args(args);
    let out = cmd.output().map_err(|e| NetError::Other(format!("iptables: {e}")))?;
    if !out.status.success() {
        return Err(NetError::Other(format!(
            "iptables {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        )));
    }
    Ok(())
}

fn enable_forwarding_in_netns(ns: &str) -> Result<(), NetError> {
    let out = Command::new("ip")
        .args([
            "netns",
            "exec",
            ns,
            "sysctl",
            "-w",
            "net.ipv4.ip_forward=1",
        ])
        .output()
        .map_err(|e| NetError::Other(format!("sysctl: {e}")))?;
    if !out.status.success() {
        return Err(NetError::Other(format!(
            "sysctl ip_forward failed: {}",
            String::from_utf8_lossy(&out.stderr)
        )));
    }
    Ok(())
}

fn guest_cidr(gateway: &str, prefix: u8) -> String {
    // MVP：网关在 /30 的 .1，guest 在 .2；用网关所在前缀构造粗 CIDR。
    // 例 172.16.0.1/30 → 172.16.0.0/30
    let Some((a, b, c, d)) = (|| {
        let parts: Vec<_> = gateway.split('.').collect();
        if parts.len() != 4 {
            return None;
        }
        Some((parts[0], parts[1], parts[2], parts[3].parse::<u32>().ok()?))
    })() else {
        return format!("{gateway}/{prefix}");
    };
    if prefix == 30 {
        let base = d & !0b11;
        return format!("{a}.{b}.{c}.{base}/{prefix}");
    }
    format!("{gateway}/{prefix}")
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
        let guest_ip = self.cfg.net.guest_ip.clone();
        let guest_port = self.cfg.net.guest_agent_port;
        let enable_veth = self.cfg.net.enable_host_veth;
        let enable_egress = self.cfg.net.enable_guest_egress;
        let pod_iface = self.cfg.net.pod_iface.clone();
        let sid = sandbox_id.to_string();
        let store = self.store.clone();

        tokio::task::spawn_blocking(move || {
            let lease = store
                .lease_ip(&cidr)
                .map_err(|e| NetError::Other(e.to_string()))?;
            let mut lease = lease;
            lease.sandbox_id = ns.clone();

            let _ = run_ip_str(&["netns", "add", &ns]);
            run_ip_str(&[
                "netns", "exec", &ns, "ip", "tuntap", "add", "dev", &tap, "mode", "tap",
            ])?;
            run_ip_str(&[
                "netns",
                "exec",
                &ns,
                "ip",
                "addr",
                "add",
                &format!("{gateway}/{prefix}"),
                "dev",
                &tap,
            ])?;
            run_ip_str(&["netns", "exec", &ns, "ip", "link", "set", &tap, "up"])?;

            let mut host_veth = String::new();
            if enable_veth {
                let plan = host_veth_plan(&sid, &ns, &lease.ip, &cidr, &pod_iface, &gw_pod)
                    .map_err(NetError::Other)?;
                for args in host_veth_setup_cmds(&plan) {
                    if let Err(e) = run_ip(&args) {
                        tracing::warn!(error = %e, netns = %ns, "host veth setup step failed");
                        let _ = run_ip(&host_veth_teardown_cmds(&plan.host_veth, &plan.pod_ip)[1]);
                        return Err(e);
                    }
                }
                host_veth = plan.host_veth;
            }

            let fwd = GuestPortForward {
                pod_ip: lease.ip.clone(),
                guest_ip,
                port: guest_port,
            };
            for args in guest_port_forward_cmds(&fwd) {
                if let Err(e) = run_iptables_in_netns(&ns, &args) {
                    tracing::warn!(error = %e, netns = %ns, "guest port forward rule skipped");
                }
            }

            if enable_egress {
                let _ = enable_forwarding_in_netns(&ns);
                let gcidr = guest_cidr(&gateway, prefix);
                for args in guest_egress_cmds(&GuestEgress {
                    guest_cidr: gcidr,
                    tap_name: tap.clone(),
                }) {
                    if let Err(e) = run_iptables_in_netns(&ns, &args) {
                        tracing::warn!(error = %e, netns = %ns, "guest egress rule skipped");
                    }
                }
                for args in host_egress_cmds(&HostEgress {
                    pod_cidr: cidr.clone(),
                }) {
                    if let Err(e) = run_iptables(&args) {
                        tracing::warn!(error = %e, "host egress MASQUERADE skipped");
                    }
                }
                // 主机转发（幂等）。
                let _ = Command::new("sysctl")
                    .args(["-w", "net.ipv4.ip_forward=1"])
                    .output();
            }

            Ok(NetConfig {
                netns_path: netns_path.to_string_lossy().into_owned(),
                tap_name: tap,
                mac,
                pod_ip: lease.ip.clone(),
                gateway: gw_pod,
                lease,
                host_veth,
            })
        })
        .await
        .map_err(|e| NetError::Other(format!("blocking join: {e}")))?
    }

    async fn teardown(&self, net: &NetConfig) -> Result<(), NetError> {
        let ns = net
            .netns_path
            .rsplit('/')
            .next()
            .unwrap_or(&net.netns_path)
            .to_string();
        let lease = net.lease.clone();
        let host_veth = net.host_veth.clone();
        let pod_ip = net.pod_ip.clone();
        let store = self.store.clone();
        tokio::task::spawn_blocking(move || {
            if !host_veth.is_empty() {
                for args in host_veth_teardown_cmds(&host_veth, &pod_ip) {
                    let _ = run_ip(&args);
                }
            }
            let _ = run_ip_str(&["netns", "del", &ns]);
            store
                .release_ip(&lease)
                .map_err(|e| NetError::Other(e.to_string()))?;
            Ok(())
        })
        .await
        .map_err(|e| NetError::Other(format!("blocking join: {e}")))?
    }
}

#[allow(dead_code)]
fn _ensure_lease_used() -> Option<IpLease> {
    None
}
