//! 每沙箱 netns + tap + host veth，并把 PodIP:guest-port 转发到 microVM。

use std::process::Command;
use std::sync::Arc;

use async_trait::async_trait;
use oas_config::Config;
use oas_store::Store;
use oas_types::IpLease;

use crate::cni::{host_veth_plan, host_veth_setup_cmds, host_veth_teardown_cmds};
use crate::forward::{
    GuestEgress, GuestPortForward, HostEgress, guest_egress_cmds, guest_port_forward_cmds,
    host_egress_cmds,
};
use crate::{NetConfig, NetError, NetworkManager};

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
    let output = Command::new("ip")
        .args(args)
        .output()
        .map_err(|error| NetError::Other(format!("ip {}: {error}", args.join(" "))))?;
    if output.status.success() {
        return Ok(());
    }
    Err(NetError::Other(format!(
        "ip {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    )))
}

fn run_ip_str(args: &[&str]) -> Result<(), NetError> {
    run_ip(
        &args
            .iter()
            .map(|arg| (*arg).to_string())
            .collect::<Vec<_>>(),
    )
}

fn run_iptables(args: &[String]) -> Result<(), NetError> {
    let output = Command::new("iptables")
        .args(["-w", "5"])
        .args(args)
        .output()
        .map_err(|error| NetError::Other(format!("iptables: {error}")))?;
    if output.status.success() {
        return Ok(());
    }
    Err(NetError::Other(format!(
        "iptables {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    )))
}

fn run_iptables_in_netns(namespace: &str, args: &[String]) -> Result<(), NetError> {
    let output = Command::new("ip")
        .args(["netns", "exec", namespace, "iptables", "-w", "5"])
        .args(args)
        .output()
        .map_err(|error| NetError::Other(format!("netns iptables: {error}")))?;
    if output.status.success() {
        return Ok(());
    }
    Err(NetError::Other(format!(
        "netns iptables {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    )))
}

fn enable_forwarding_in_netns(namespace: &str) -> Result<(), NetError> {
    let output = Command::new("ip")
        .args([
            "netns",
            "exec",
            namespace,
            "sysctl",
            "-w",
            "net.ipv4.ip_forward=1",
        ])
        .output()
        .map_err(|error| NetError::Other(format!("netns sysctl: {error}")))?;
    if output.status.success() {
        return Ok(());
    }
    Err(NetError::Other(format!(
        "netns ip_forward failed: {}",
        String::from_utf8_lossy(&output.stderr)
    )))
}

fn guest_cidr(gateway: &str, prefix: u8) -> String {
    let parts: Vec<_> = gateway.split('.').collect();
    if prefix == 30 && parts.len() == 4 {
        if let Ok(last) = parts[3].parse::<u8>() {
            return format!(
                "{}.{}.{}.{}/{prefix}",
                parts[0],
                parts[1],
                parts[2],
                last & !3
            );
        }
    }
    format!("{gateway}/{prefix}")
}

#[async_trait]
impl NetworkManager for NetManager {
    async fn setup(&self, sandbox_id: &str) -> Result<NetConfig, NetError> {
        let namespace = self.cfg.netns_name(sandbox_id);
        let netns_path = self.cfg.netns_path(sandbox_id);
        let tap = self.cfg.net.tap_name.clone();
        let gateway = self.cfg.net.tap_gateway.clone();
        let prefix = self.cfg.net.tap_prefix;
        let mac = self.cfg.net.guest_mac.clone();
        let pod_cidr = self.cfg.net.pod_cidr.clone();
        let pod_gateway = self.cfg.net.pod_gateway.clone();
        let guest_ip = self.cfg.net.guest_ip.clone();
        let guest_port = self.cfg.net.guest_agent_port;
        let enable_veth = self.cfg.net.enable_host_veth;
        let enable_egress = self.cfg.net.enable_guest_egress;
        let pod_iface = self.cfg.net.pod_iface.clone();
        let sandbox_id = sandbox_id.to_string();
        let store = self.store.clone();

        tokio::task::spawn_blocking(move || {
            let mut lease = store
                .lease_ip(&pod_cidr)
                .map_err(|error| NetError::Other(error.to_string()))?;
            lease.sandbox_id = namespace.clone();

            let _ = run_ip_str(&["netns", "add", &namespace]);
            run_ip_str(&[
                "netns", "exec", &namespace, "ip", "tuntap", "add", "dev", &tap, "mode", "tap",
            ])?;
            run_ip_str(&[
                "netns",
                "exec",
                &namespace,
                "ip",
                "addr",
                "add",
                &format!("{gateway}/{prefix}"),
                "dev",
                &tap,
            ])?;
            run_ip_str(&["netns", "exec", &namespace, "ip", "link", "set", &tap, "up"])?;

            let mut host_veth = String::new();
            if enable_veth {
                let plan = host_veth_plan(
                    &sandbox_id,
                    &namespace,
                    &lease.ip,
                    &pod_cidr,
                    &pod_iface,
                    &pod_gateway,
                )
                .map_err(NetError::Other)?;
                for command in host_veth_setup_cmds(&plan) {
                    if let Err(error) = run_ip(&command) {
                        tracing::warn!(%error, netns = %namespace, "host veth setup failed");
                        for cleanup in host_veth_teardown_cmds(&plan.host_veth, &plan.pod_ip) {
                            let _ = run_ip(&cleanup);
                        }
                        let _ = run_ip_str(&["netns", "del", &namespace]);
                        let _ = store.release_ip(&lease);
                        return Err(error);
                    }
                }
                host_veth = plan.host_veth;
            }

            for command in guest_port_forward_cmds(&GuestPortForward {
                pod_ip: lease.ip.clone(),
                guest_ip,
                port: guest_port,
            }) {
                run_iptables_in_netns(&namespace, &command)?;
            }

            if enable_egress {
                enable_forwarding_in_netns(&namespace)?;
                for command in guest_egress_cmds(&GuestEgress {
                    guest_cidr: guest_cidr(&gateway, prefix),
                    tap_name: tap.clone(),
                }) {
                    run_iptables_in_netns(&namespace, &command)?;
                }
                for command in host_egress_cmds(&HostEgress {
                    pod_cidr: pod_cidr.clone(),
                }) {
                    if let Err(error) = run_iptables(&command) {
                        tracing::warn!(%error, "host egress MASQUERADE skipped");
                    }
                }
                let _ = Command::new("sysctl")
                    .args(["-w", "net.ipv4.ip_forward=1"])
                    .output();
            }

            Ok(NetConfig {
                netns_path: netns_path.to_string_lossy().into_owned(),
                tap_name: tap,
                mac,
                pod_ip: lease.ip.clone(),
                gateway: pod_gateway,
                lease,
                host_veth,
            })
        })
        .await
        .map_err(|error| NetError::Other(format!("blocking join: {error}")))?
    }

    async fn teardown(&self, net: &NetConfig) -> Result<(), NetError> {
        let namespace = net
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
                for command in host_veth_teardown_cmds(&host_veth, &pod_ip) {
                    let _ = run_ip(&command);
                }
            }
            let _ = run_ip_str(&["netns", "del", &namespace]);
            store
                .release_ip(&lease)
                .map_err(|error| NetError::Other(error.to_string()))
        })
        .await
        .map_err(|error| NetError::Other(format!("blocking join: {error}")))?
    }
}

#[allow(dead_code)]
fn _ensure_lease_used() -> Option<IpLease> {
    None
}
