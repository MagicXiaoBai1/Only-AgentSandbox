//! PodIP 到 guest-agent 的转发，以及 guest 出向 NAT 规则。

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuestPortForward {
    pub pod_ip: String,
    pub guest_ip: String,
    pub port: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuestEgress {
    pub guest_cidr: String,
    pub tap_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostEgress {
    pub pod_cidr: String,
}

pub fn guest_port_forward_cmds(fwd: &GuestPortForward) -> Vec<Vec<String>> {
    vec![
        vec![
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
        ],
        vec![
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
        ],
    ]
}

pub fn guest_egress_cmds(egress: &GuestEgress) -> Vec<Vec<String>> {
    vec![
        vec![
            "-A".into(),
            "FORWARD".into(),
            "-i".into(),
            egress.tap_name.clone(),
            "-j".into(),
            "ACCEPT".into(),
        ],
        vec![
            "-A".into(),
            "FORWARD".into(),
            "-o".into(),
            egress.tap_name.clone(),
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
            egress.guest_cidr.clone(),
            "!".into(),
            "-d".into(),
            egress.guest_cidr.clone(),
            "-j".into(),
            "MASQUERADE".into(),
        ],
    ]
}

pub fn host_egress_cmds(egress: &HostEgress) -> Vec<Vec<String>> {
    vec![vec![
        "-t".into(),
        "nat".into(),
        "-A".into(),
        "POSTROUTING".into(),
        "-s".into(),
        egress.pod_cidr.clone(),
        "-j".into(),
        "MASQUERADE".into(),
    ]]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forwards_guest_agent_port() {
        let commands = guest_port_forward_cmds(&GuestPortForward {
            pod_ip: "10.42.0.17".into(),
            guest_ip: "172.16.0.2".into(),
            port: 10000,
        });
        assert!(
            commands[0]
                .windows(2)
                .any(|pair| pair == ["--to-destination", "172.16.0.2:10000"])
        );
        assert!(commands[1].contains(&"MASQUERADE".to_string()));
    }
}
