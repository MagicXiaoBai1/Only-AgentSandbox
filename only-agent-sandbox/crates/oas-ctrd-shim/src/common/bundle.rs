// Task 路径: 从 bundle 的 OCI `config.json` 提炼 shim 需要的字段。
//
// containerd 1.6.33 的 Task 路径不像 2.x Sandbox API 那样把 netns 当请求一等字段传,
// 而是把它塞进 bundle OCI spec 的 `spec.linux.Namespaces[type=network].Path`
// (CRI 的 "updateNetNamespacePath")。同理, 这条 task 是 pod 还是业务容器, 它属于哪个
// pod) 都编码在 OCI `annotations` 里。故本 helper 读一次 `config.json` 拿齐:
// - `container_type`: `io.kubernetes.cri.container-type` (`sandbox` / `container`)。
// - `sandbox_id`: `io.kubernetes.cri.sandbox-id` (container-Task 指回所属 pod; grouping 用)。
// - `netns_path`: network 类型 namespace 的 `path` (交 driver 建 tap0)。
// - `type_id`: pod annotation `agent-sandbox/type` (选 bundle type; 见下方注意)。
//
// 两条协议路径的 netns 来源不同 (Sandbox 是请求字段、Task 在 spec 里), 但拿到 netns
// 之后统一交 driver——本 helper 属 common, 供 Task 路径的 start 归组与 create 复用。
//
// **type_id 透传注意**: `agent-sandbox/type` 是 pod annotation, containerd CRI 默认 **不**
// 把任意 pod annotation 写进 OCI spec (只注入它自己的 `io.kubernetes.cri.*`)。须在
// containerd config.toml 配 `pod_annotations = ["agent-sandbox/.*"]` 才会进 config.json。
// 未配 / 裸 ctr run 时 `type_id` 为 None, 调用方回落 0 (见 TaskService::create)。

use std::path::Path;

use serde::Deserialize;

/// CRI annotation: 这条 task 的类型 (`sandbox` = pod/pause, `container` = 业务容器)。
pub const ANNOTATION_CONTAINER_TYPE: &str = "io.kubernetes.cri.container-type";
/// CRI annotation: 业务容器指回所属 pod 的 sandbox id (container-Task 才有)。
pub const ANNOTATION_SANDBOX_ID: &str = "io.kubernetes.cri.sandbox-id";
/// OAS pod annotation: sandbox bundle type (选 `$artifacts/snapshots/<bundle>`)。
/// 需 containerd `pod_annotations` 透传才进 config.json (见模块注释)。
pub const ANNOTATION_TYPE: &str = "agent-sandbox/type";

/// container-type annotation 的两种取值。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerType {
    /// pod/pause 那条 task (对 DAS = 起/管一个 microVM)。
    Sandbox,
    /// 业务容器那条 task (P1 = 假 exit)。
    Container,
}

/// 从 bundle `config.json` 提炼出的 Task 路径入参。
#[derive(Debug, Clone)]
pub struct BundleSpec {
    /// `io.kubernetes.cri.container-type`; 缺省视为 Sandbox (对齐 CRI: 无该 annotation 的
    /// 裸 `ctr run` 也当 pod 处理, 走 SandboxVm)。
    pub container_type: ContainerType,
    /// `io.kubernetes.cri.sandbox-id` (container-Task 才有; sandbox-Task 为 None)。
    pub sandbox_id: Option<String>,
    /// network namespace 的 path (无则 None, driver 侧当 不进 netns)。
    pub netns_path: Option<String>,
    /// `agent-sandbox/type` 解析出的 bundle type_id; None 表示未透传 (回落 0)。
    pub type_id: Option<u8>,
}

// ---- OCI config.json 局部反序列化 (只取需要的字段) -------------------------

#[derive(Debug, Deserialize)]
struct OciSpec {
    #[serde(default)]
    annotations: std::collections::HashMap<String, String>,
    #[serde(default)]
    linux: Option<OciLinux>,
}

#[derive(Debug, Deserialize)]
struct OciLinux {
    #[serde(default)]
    namespaces: Vec<OciNamespace>,
}

#[derive(Debug, Deserialize)]
struct OciNamespace {
    #[serde(rename = "type")]
    ns_type: String,
    #[serde(default)]
    path: String,
}

/// 读 `<bundle>/config.json` 并提炼 [`BundleSpec`]。
pub fn load_bundle_spec(bundle_dir: &str) -> Result<BundleSpec, String> {
    let path = Path::new(bundle_dir).join("config.json");
    let bytes = std::fs::read(&path)
        .map_err(|e| format!("read {}: {}", path.display(), e))?;
    parse_bundle_spec(&bytes)
}

/// 从 config.json 字节流解析 (抽出便于单测)。
pub fn parse_bundle_spec(bytes: &[u8]) -> Result<BundleSpec, String> {
    let spec: OciSpec =
        serde_json::from_slice(bytes).map_err(|e| format!("parse config.json: {}", e))?;

    let container_type = match spec.annotations.get(ANNOTATION_CONTAINER_TYPE).map(String::as_str) {
        Some("container") => ContainerType::Container,
        // "sandbox" 或缺省都当 Sandbox (即 ctr run 无 annotation = pod)。
        _ => ContainerType::Sandbox,
    };
    let sandbox_id = spec
        .annotations
        .get(ANNOTATION_SANDBOX_ID)
        .filter(|s| !s.is_empty())
        .cloned();
    let netns_path = spec
        .linux
        .as_ref()
        .and_then(|l| l.namespaces.iter().find(|n| n.ns_type == "network"))
        .map(|n| n.path.clone())
        .filter(|p| !p.is_empty());
    // agent-sandbox/type: pod annotation, 须 containerd pod_annotations 透传才在。
    let type_id = spec
        .annotations
        .get(ANNOTATION_TYPE)
        .filter(|s| !s.is_empty())
        .and_then(|s| s.parse::<u8>().ok());

    Ok(BundleSpec {
        container_type,
        sandbox_id,
        netns_path,
        type_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sandbox_task_with_netns() {
        let json = br#"{
            "annotations": {
                "io.kubernetes.cri.container-type": "sandbox"
            },
            "linux": {
                "namespaces": [
                    {"type": "network", "path": "/var/run/netns/cni-abc"},
                    {"type": "pid"}
                ]
            }
        }"#;
        let s = parse_bundle_spec(json).unwrap();
        assert_eq!(s.container_type, ContainerType::Sandbox);
        assert_eq!(s.sandbox_id, None);
        assert_eq!(s.netns_path.as_deref(), Some("/var/run/netns/cni-abc"));
    }

    #[test]
    fn container_task_references_pod() {
        let json = br#"{
            "annotations": {
                "io.kubernetes.cri.container-type": "container",
                "io.kubernetes.cri.sandbox-id": "pod-xyz"
            }
        }"#;
        let s = parse_bundle_spec(json).unwrap();
        assert_eq!(s.container_type, ContainerType::Container);
        assert_eq!(s.sandbox_id.as_deref(), Some("pod-xyz"));
        assert_eq!(s.netns_path, None);
    }

    #[test]
    fn bare_spec_defaults_to_sandbox() {
        // 裸 ctr run: 无 CRI annotation。
        let json = br#"{"ociVersion": "1.0.0"}"#;
        let s = parse_bundle_spec(json).unwrap();
        assert_eq!(s.container_type, ContainerType::Sandbox);
        assert_eq!(s.sandbox_id, None);
        assert_eq!(s.netns_path, None);
    }

    #[test]
    fn empty_netns_path_is_none() {
        let json = br#"{
            "linux": {"namespaces": [{"type": "network", "path": ""}]}
        }"#;
        let s = parse_bundle_spec(json).unwrap();
        assert_eq!(s.netns_path, None);
    }

    #[test]
    fn type_id_parsed_from_annotation() {
        let json = br#"{
            "annotations": {
                "io.kubernetes.cri.container-type": "sandbox",
                "agent-sandbox/type": "1"
            }
        }"#;
        let s = parse_bundle_spec(json).unwrap();
        assert_eq!(s.type_id, Some(1));
    }

    #[test]
    fn type_id_absent_is_none() {
        // 裸 ctr run / 未配 pod_annotations: annotation 不在 → None (调用方回落 0)。
        let json = br#"{"ociVersion": "1.0.0"}"#;
        let s = parse_bundle_spec(json).unwrap();
        assert_eq!(s.type_id, None);
    }

    #[test]
    fn type_id_invalid_is_none() {
        let json = br#"{"annotations": {"agent-sandbox/type": "not-a-number"}}"#;
        let s = parse_bundle_spec(json).unwrap();
        assert_eq!(s.type_id, None);
    }
}