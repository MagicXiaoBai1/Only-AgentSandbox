//! A1 网络规则：入向 guest-agent 端口转发 + guest 出向 NAT。

/// 单条 guest 端口转发意图（集群 → guest-agent）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuestPortForward {
    pub pod_ip: String,
    pub guest_ip: String,
    pub port: u16,
}

/// guest 出向意图（guest → 外网）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuestEgress {
    /// guest/tap 网段，如 `172.16.0.0/30`。
    pub guest_cidr: String,
    /// tap 设备名。
    pub tap_name: String,
}

/// 主机侧对 pod CIDR 的 MASQUERADE（跨 netns 出网）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostEgress {
    pub pod_cidr: String,
}

/// 生成在 `ip netns exec <ns>` 内执行的 iptables 参数（不含 `iptables` 本征）。
///
/// 规则：
/// 1. PREROUTING DNAT：`PodIP:port → guest_ip:port`
/// 2. POSTROUTING MASQUERADE：guest 回包经网关时伪装
pub fn guest_port_forward_cmds(fwd: &GuestPortForward) -> Vec<Vec<String>> {
    let dnat = vec![
        "-t".into(),
        "nat".into(),
        "-A".into(),
        "PREROUTING".into(),
        "-p".into(),
        "tcp".into(),
        "-d".into(),
        fwd.pod_ip.clone(),
        "--dport".into(),
        fwd.port.to_string(),
        "-j".into(),
        "DNAT".into(),
        "--to-destination".into(),
        format!("{}:{}", fwd.guest_ip, fwd.port),
    ];
    let masq = vec![
        "-t".into(),
        "nat".into(),
        "-A".into(),
        "POSTROUTING".into(),
        "-d".into(),
        fwd.guest_ip.clone(),
        "-p".into(),
        "tcp".into(),
        "--dport".into(),
        fwd.port.to_string(),
        "-j".into(),
        "MASQUERADE".into(),
    ];
    vec![dnat, masq]
}

/// netns 内：允许 tap 转发并为 guest 网段做 SNAT（经 eth0/host_veth 出网）。
pub fn guest_egress_cmds(e: &GuestEgress) -> Vec<Vec<String>> {
    vec![
        vec![
            "-A".into(),
            "FORWARD".into(),
            "-i".into(),
            e.tap_name.clone(),
            "-j".into(),
            "ACCEPT".into(),
        ],
        vec![
            "-A".into(),
            "FORWARD".into(),
            "-o".into(),
            e.tap_name.clone(),
            "-m".into(),
            "conntrack".into(),
            "--ctstate".into(),
            "RELATED,ESTABLISHED".into(),
            "-j".into(),
            "ACCEPT".into(),
        ],
        vec![
            "-t".into(),
            "nat".into(),
            "-A".into(),
            "POSTROUTING".into(),
            "-s".into(),
            e.guest_cidr.clone(),
            "!".into(),
            "-d".into(),
            e.guest_cidr.clone(),
            "-j".into(),
            "MASQUERADE".into(),
        ],
    ]
}

/// 主机侧：MASQUERADE pod CIDR（同节点出网 / 回集群）。
pub fn host_egress_cmds(e: &HostEgress) -> Vec<Vec<String>> {
    vec![vec![
        "-t".into(),
        "nat".into(),
        "-A".into(),
        "POSTROUTING".into(),
        "-s".into(),
        e.pod_cidr.clone(),
        "-j".into(),
        "MASQUERADE".into(),
    ]]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guest_agent_forward_targets_port_10000() {
        let cmds = guest_port_forward_cmds(&GuestPortForward {
            pod_ip: "10.244.0.7".into(),
            guest_ip: "172.16.0.2".into(),
            port: 10000,
        });
        assert_eq!(cmds.len(), 2);
        assert!(cmds[0].windows(2).any(|w| w == ["--dport", "10000"]));
        assert!(cmds[0]
            .windows(2)
            .any(|w| w == ["--to-destination", "172.16.0.2:10000"]));
        assert!(cmds[1].contains(&"MASQUERADE".to_string()));
    }

    #[test]
    fn guest_egress_masquerades_guest_cidr() {
        let cmds = guest_egress_cmds(&GuestEgress {
            guest_cidr: "172.16.0.0/30".into(),
            tap_name: "tapH0".into(),
        });
        assert!(cmds.iter().any(|c| c.contains(&"FORWARD".to_string())));
        assert!(cmds
            .iter()
            .any(|c| c.windows(2).any(|w| w == ["-s", "172.16.0.0/30"])));
    }

    #[test]
    fn host_egress_masquerades_pod_cidr() {
        let cmds = host_egress_cmds(&HostEgress {
            pod_cidr: "10.244.0.0/24".into(),
        });
        assert_eq!(cmds.len(), 1);
        assert!(cmds[0].windows(2).any(|w| w == ["-s", "10.244.0.0/24"]));
    }
}
