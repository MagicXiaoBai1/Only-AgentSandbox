//! 主机侧 veth：把 IPAM 租到的 PodIP 挂进沙箱 netns。

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostVethPlan {
    pub netns_name: String,
    pub host_veth: String,
    pub peer_veth: String,
    pub pod_iface: String,
    pub pod_ip: String,
    pub prefix: u8,
    pub host_gateway_ip: String,
}

pub fn cidr_prefix(cidr: &str) -> Result<u8, String> {
    let Some((_, prefix)) = cidr.split_once('/') else {
        return Err(format!("cidr missing prefix: {cidr}"));
    };
    prefix
        .parse::<u8>()
        .map_err(|_| format!("invalid cidr prefix: {cidr}"))
}

pub fn veth_names(sandbox_id: &str) -> (String, String) {
    let mut hash: u32 = 2_166_136_261;
    for byte in sandbox_id.as_bytes() {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(16_777_619);
    }
    let digest = format!("{hash:08x}");
    (format!("oasv{digest}"), format!("oasp{digest}"))
}

pub fn host_veth_plan(
    sandbox_id: &str,
    netns_name: &str,
    pod_ip: &str,
    pod_cidr: &str,
    pod_iface: &str,
    host_gateway_ip: &str,
) -> Result<HostVethPlan, String> {
    let prefix = cidr_prefix(pod_cidr)?;
    let (host_veth, peer_veth) = veth_names(sandbox_id);
    Ok(HostVethPlan {
        netns_name: netns_name.into(),
        host_veth,
        peer_veth,
        pod_iface: pod_iface.into(),
        pod_ip: pod_ip.into(),
        prefix,
        host_gateway_ip: host_gateway_ip.into(),
    })
}

pub fn host_veth_setup_cmds(plan: &HostVethPlan) -> Vec<Vec<String>> {
    let ns = &plan.netns_name;
    let host = &plan.host_veth;
    let peer = &plan.peer_veth;
    let iface = &plan.pod_iface;
    vec![
        vec![
            "link".into(),
            "add".into(),
            host.clone(),
            "type".into(),
            "veth".into(),
            "peer".into(),
            "name".into(),
            peer.clone(),
        ],
        vec![
            "link".into(),
            "set".into(),
            peer.clone(),
            "netns".into(),
            ns.clone(),
        ],
        vec![
            "netns".into(),
            "exec".into(),
            ns.clone(),
            "ip".into(),
            "link".into(),
            "set".into(),
            peer.clone(),
            "name".into(),
            iface.clone(),
        ],
        vec![
            "netns".into(),
            "exec".into(),
            ns.clone(),
            "ip".into(),
            "addr".into(),
            "add".into(),
            format!("{}/{}", plan.pod_ip, plan.prefix),
            "dev".into(),
            iface.clone(),
        ],
        vec![
            "netns".into(),
            "exec".into(),
            ns.clone(),
            "ip".into(),
            "link".into(),
            "set".into(),
            iface.clone(),
            "up".into(),
        ],
        vec![
            "addr".into(),
            "add".into(),
            format!("{}/{}", plan.host_gateway_ip, plan.prefix),
            "dev".into(),
            host.clone(),
        ],
        vec!["link".into(), "set".into(), host.clone(), "up".into()],
        vec![
            "netns".into(),
            "exec".into(),
            ns.clone(),
            "ip".into(),
            "route".into(),
            "replace".into(),
            "default".into(),
            "via".into(),
            plan.host_gateway_ip.clone(),
            "dev".into(),
            iface.clone(),
        ],
        vec![
            "route".into(),
            "replace".into(),
            plan.pod_ip.clone(),
            "dev".into(),
            host.clone(),
        ],
    ]
}

pub fn host_veth_teardown_cmds(host_veth: &str, pod_ip: &str) -> Vec<Vec<String>> {
    vec![
        vec!["route".into(), "del".into(), pod_ip.into()],
        vec!["link".into(), "del".into(), host_veth.into()],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_assigns_pod_ip_and_route() {
        let plan = host_veth_plan(
            "sb1",
            "oas-sb1",
            "10.42.0.17",
            "10.42.0.0/24",
            "eth0",
            "10.42.0.254",
        )
        .unwrap();
        let commands = host_veth_setup_cmds(&plan);
        let flat: Vec<_> = commands.iter().flatten().map(String::as_str).collect();
        assert!(flat.contains(&"10.42.0.17/24"));
        assert!(flat.contains(&"eth0"));
        assert!(
            commands
                .iter()
                .any(|cmd| cmd.windows(2).any(|pair| pair == ["route", "replace"]))
        );
    }

    #[test]
    fn names_are_short_stable_and_distinct() {
        let first = veth_names("sandbox-42");
        assert_eq!(first, veth_names("sandbox-42"));
        assert!(first.0.len() <= 15 && first.1.len() <= 15);
        assert_ne!(first.0, first.1);
    }
}
