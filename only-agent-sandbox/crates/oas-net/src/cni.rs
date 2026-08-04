//! A1 主机侧 veth：把 IPAM 租到的 PodIP 挂进沙箱 netns 的 eth0。
//!
//! 完整集群 CNI（Calico/Flannel 插件调用）后续可替换本模块的执行面；
//! 命令计划保持稳定，便于单测与逐步切换。

use std::path::PathBuf;

/// 一次沙箱网络附着计划（不含 tap/guest 侧）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostVethPlan {
    pub netns_name: String,
    pub host_veth: String,
    pub peer_veth: String,
    pub pod_iface: String,
    pub pod_ip: String,
    pub prefix: u8,
    /// 主机侧 veth IP（作 netns 默认网关），通常为 pod_gateway。
    pub host_gateway_ip: String,
}

/// 解析 CIDR 前缀长度；非法则返回错误字符串。
pub fn cidr_prefix(cidr: &str) -> Result<u8, String> {
    let Some((_, pfx)) = cidr.split_once('/') else {
        return Err(format!("cidr missing prefix: {cidr}"));
    };
    pfx.parse::<u8>()
        .map_err(|_| format!("invalid cidr prefix: {cidr}"))
}

/// 由 sandbox_id 派生短且合法的 veth 名（Linux IFNAMSIZ=15）。
pub fn veth_names(sandbox_id: &str) -> (String, String) {
    let digest = {
        let mut h: u32 = 2166136261;
        for b in sandbox_id.as_bytes() {
            h ^= u32::from(*b);
            h = h.wrapping_mul(16777619);
        }
        format!("{h:08x}")
    };
    // host: oasvXXXXXXXX (12), peer: 临时名再 rename 为 eth0
    let host = format!("oasv{}", &digest[..8]);
    let peer = format!("oasp{}", &digest[..8]);
    (host, peer)
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
        netns_name: netns_name.to_string(),
        host_veth,
        peer_veth,
        pod_iface: pod_iface.to_string(),
        pod_ip: pod_ip.to_string(),
        prefix,
        host_gateway_ip: host_gateway_ip.to_string(),
    })
}

/// 生成实现 `HostVethPlan` 的 `ip` 参数列表（每条为一次 `ip` 调用的 argv）。
pub fn host_veth_setup_cmds(plan: &HostVethPlan) -> Vec<Vec<String>> {
    let ns = &plan.netns_name;
    let host = &plan.host_veth;
    let peer = &plan.peer_veth;
    let iface = &plan.pod_iface;
    let addr = format!("{}/{}", plan.pod_ip, plan.prefix);
    let gw_addr = format!("{}/{}", plan.host_gateway_ip, plan.prefix);
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
            addr,
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
        // 主机侧网关 IP，供 netns default route。
        vec![
            "addr".into(),
            "add".into(),
            gw_addr,
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
        // 主机经 host-veth 直达该 PodIP。
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
        vec!["route".into(), "del".into(), pod_ip.to_string()],
        vec!["link".into(), "del".into(), host_veth.to_string()],
    ]
}

/// CNI 配置目录探测结果（后续接真实插件）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CniPaths {
    pub bin_dir: PathBuf,
    pub conf_dir: PathBuf,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cidr_prefix_parses() {
        assert_eq!(cidr_prefix("10.244.0.0/24").unwrap(), 24);
        assert!(cidr_prefix("10.244.0.0").is_err());
    }

    #[test]
    fn veth_names_are_short_and_stable() {
        let (a, b) = veth_names("sandbox-42");
        let (c, d) = veth_names("sandbox-42");
        assert_eq!((a.clone(), b.clone()), (c, d));
        assert!(a.len() <= 15 && b.len() <= 15);
        assert_ne!(a, b);
    }

    #[test]
    fn host_veth_plan_assigns_pod_ip_to_eth0() {
        let plan = host_veth_plan(
            "sb1",
            "oas-sb1",
            "10.244.0.7",
            "10.244.0.0/24",
            "eth0",
            "10.244.0.254",
        )
        .unwrap();
        let cmds = host_veth_setup_cmds(&plan);
        let flat: Vec<String> = cmds.iter().flatten().cloned().collect();
        assert!(flat.windows(2).any(|w| w == ["10.244.0.7/24", "dev"]));
        assert!(flat.iter().any(|s| s == "eth0"));
        assert!(cmds.iter().any(|c| c.windows(2).any(|w| w == ["route", "replace"])));
        assert!(flat.windows(2).any(|w| w == ["via", "10.244.0.254"]));
    }
}
