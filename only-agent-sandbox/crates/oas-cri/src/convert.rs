//! proto ⇄ 内部模型 转换（§3.1 `convert.rs`）。
//!
//! 红线（§6-1）：metadata / labels / annotations 全链路原样透传——存即原样、取即原样，
//! 禁用 `Default` 覆盖。本模块只做直接拷贝，不做语义改写。

use std::collections::HashMap;

use oas_manager::{CreateContainerRequest, CreateSandboxRequest, ImageInfo, OasError};
use oas_types::{
    ContainerExitReason, ContainerMetadata as DomainContainerMeta, DnsConfig, KeyValue,
    LinuxResources, Mount as DomainMount, MountPropagation as DomainMountProp,
    SandboxMetadata as DomainSandboxMeta,
};

use crate::runtime::v1 as pb;

// ---------------------------------------------------------------------------
// metadata（原样透传）
// ---------------------------------------------------------------------------

pub fn pod_metadata_to_domain(m: &pb::PodSandboxMetadata) -> DomainSandboxMeta {
    DomainSandboxMeta {
        name: m.name.clone(),
        namespace: m.namespace.clone(),
        uid: m.uid.clone(),
        attempt: m.attempt,
    }
}

pub fn container_metadata_to_domain(m: &pb::ContainerMetadata) -> DomainContainerMeta {
    DomainContainerMeta {
        name: m.name.clone(),
        attempt: m.attempt,
    }
}

pub fn sandbox_metadata_from_domain(m: &DomainSandboxMeta) -> pb::PodSandboxMetadata {
    pb::PodSandboxMetadata {
        name: m.name.clone(),
        uid: m.uid.clone(),
        namespace: m.namespace.clone(),
        attempt: m.attempt,
    }
}

pub fn container_metadata_from_domain(m: &DomainContainerMeta) -> pb::ContainerMetadata {
    pb::ContainerMetadata {
        name: m.name.clone(),
        attempt: m.attempt,
    }
}

// ---------------------------------------------------------------------------
// state 映射
// ---------------------------------------------------------------------------

pub fn sandbox_state(s: oas_types::SandboxState) -> i32 {
    match s {
        oas_types::SandboxState::Ready => pb::PodSandboxState::SandboxReady as i32,
        oas_types::SandboxState::NotReady => pb::PodSandboxState::SandboxNotready as i32,
    }
}

pub fn container_state(s: oas_types::ContainerState) -> i32 {
    match s {
        oas_types::ContainerState::Created => pb::ContainerState::ContainerCreated as i32,
        oas_types::ContainerState::Running => pb::ContainerState::ContainerRunning as i32,
        oas_types::ContainerState::Exited => pb::ContainerState::ContainerExited as i32,
        oas_types::ContainerState::Unknown => pb::ContainerState::ContainerUnknown as i32,
    }
}

fn exit_reason_str(r: Option<ContainerExitReason>) -> &'static str {
    match r {
        None => "",
        Some(ContainerExitReason::Completed) => "Completed",
        Some(ContainerExitReason::Error) => "Error",
        Some(ContainerExitReason::OomKilled) => "OOMKilled",
    }
}

// ---------------------------------------------------------------------------
// 小工具类型
// ---------------------------------------------------------------------------

pub fn dns_config_to_domain(d: &pb::DnsConfig) -> DnsConfig {
    DnsConfig {
        servers: d.servers.clone(),
        searches: d.searches.clone(),
        options: d.options.clone(),
    }
}

fn keyvalue_to_domain(kv: &pb::KeyValue) -> KeyValue {
    KeyValue {
        key: kv.key.clone(),
        value: kv.value.clone(),
    }
}

fn mount_propagation_to_domain(p: i32) -> DomainMountProp {
    match p {
        x if x == pb::MountPropagation::PropagationHostToContainer as i32 => {
            DomainMountProp::HostToContainer
        }
        x if x == pb::MountPropagation::PropagationBidirectional as i32 => {
            DomainMountProp::Bidirectional
        }
        _ => DomainMountProp::Private,
    }
}

pub fn mount_to_domain(m: &pb::Mount) -> DomainMount {
    DomainMount {
        container_path: m.container_path.clone(),
        host_path: m.host_path.clone(),
        read_only: m.readonly,
        propagation: mount_propagation_to_domain(m.propagation),
    }
}

fn linux_resources_to_domain(r: &pb::LinuxContainerResources) -> LinuxResources {
    LinuxResources {
        cpu_shares: if r.cpu_shares > 0 {
            Some(r.cpu_shares as u64)
        } else {
            None
        },
        memory_limit_bytes: if r.memory_limit_in_bytes > 0 {
            Some(r.memory_limit_in_bytes)
        } else {
            None
        },
    }
}

fn image_spec(image: &str) -> pb::ImageSpec {
    pb::ImageSpec {
        image: image.into(),
        ..Default::default()
    }
}

// ---------------------------------------------------------------------------
// RunPodSandbox: PodSandboxConfig → CreateSandboxRequest
// ---------------------------------------------------------------------------

const ANNOT_TYPE: &str = "agent-sandbox/type";
const ANNOT_CLOUD_DISK: &str = "agent-sandbox/cloud-disk";
const ANNOT_RW_SIZE: &str = "agent-sandbox/rw-size";

/// 从 annotations 解析 `type_id`（§9）。
fn parse_type_id(annotations: &HashMap<String, String>) -> Result<u8, OasError> {
    let raw = annotations
        .get(ANNOT_TYPE)
        .ok_or_else(|| OasError::InvalidArgument(format!("missing annotation {ANNOT_TYPE}")))?;
    raw.parse::<u8>()
        .map_err(|_| OasError::InvalidArgument(format!("invalid {ANNOT_TYPE}: {raw}")))
}

pub fn sandbox_config_to_create_req(
    cfg: &pb::PodSandboxConfig,
    runtime_handler: &str,
) -> Result<CreateSandboxRequest, OasError> {
    let metadata = cfg
        .metadata
        .as_ref()
        .ok_or_else(|| OasError::InvalidArgument("missing sandbox metadata".into()))?;
    let type_id = parse_type_id(&cfg.annotations)?;
    let cloud_disk_ref = cfg
        .annotations
        .get(ANNOT_CLOUD_DISK)
        .filter(|s| !s.is_empty())
        .cloned();
    let rw_size = cfg
        .annotations
        .get(ANNOT_RW_SIZE)
        .and_then(|s| s.parse::<u64>().ok());
    let cgroup_parent = cfg
        .linux
        .as_ref()
        .map(|l| l.cgroup_parent.clone())
        .unwrap_or_default();
    let handler = if runtime_handler.is_empty() {
        "oas".to_string()
    } else {
        runtime_handler.to_string()
    };

    Ok(CreateSandboxRequest {
        metadata: pod_metadata_to_domain(metadata),
        labels: cfg.labels.clone(),
        annotations: cfg.annotations.clone(),
        hostname: cfg.hostname.clone(),
        log_directory: cfg.log_directory.clone(),
        dns_config: cfg.dns_config.as_ref().map(dns_config_to_domain),
        cgroup_parent,
        type_id,
        cloud_disk_ref,
        rw_size,
        runtime_handler: handler,
    })
}

// ---------------------------------------------------------------------------
// CreateContainer: ContainerConfig → CreateContainerRequest
// ---------------------------------------------------------------------------

pub fn container_config_to_create_req(
    pod_sandbox_id: &str,
    cfg: &pb::ContainerConfig,
) -> Result<CreateContainerRequest, OasError> {
    let metadata = cfg
        .metadata
        .as_ref()
        .ok_or_else(|| OasError::InvalidArgument("missing container metadata".into()))?;
    let image = cfg
        .image
        .as_ref()
        .ok_or_else(|| OasError::InvalidArgument("missing container image".into()))?;
    let resources = cfg
        .linux
        .as_ref()
        .and_then(|l| l.resources.as_ref())
        .map(linux_resources_to_domain)
        .unwrap_or_default();

    Ok(CreateContainerRequest {
        pod_sandbox_id: pod_sandbox_id.into(),
        metadata: container_metadata_to_domain(metadata),
        image: image.image.clone(),
        command: cfg.command.clone(),
        args: cfg.args.clone(),
        working_dir: cfg.working_dir.clone(),
        envs: cfg.envs.iter().map(keyvalue_to_domain).collect(),
        mounts: cfg.mounts.iter().map(mount_to_domain).collect(),
        labels: cfg.labels.clone(),
        annotations: cfg.annotations.clone(),
        log_path: cfg.log_path.clone(),
        resources,
        tty: cfg.tty,
        stdin: cfg.stdin,
        stdin_once: cfg.stdin_once,
    })
}

// ---------------------------------------------------------------------------
// SandboxRecord → proto（PodSandboxStatus / PodSandbox）
// ---------------------------------------------------------------------------

pub fn record_to_sandbox_status(r: &oas_types::SandboxRecord) -> pb::PodSandboxStatus {
    pb::PodSandboxStatus {
        id: r.sandbox_id.clone(),
        metadata: Some(sandbox_metadata_from_domain(&r.metadata)),
        state: sandbox_state(r.state),
        created_at: secs_to_ns(r.created_at),
        network: Some(pb::PodSandboxNetworkStatus {
            ip: r.pod_ip.clone(),
            additional_ips: Vec::new(),
        }),
        linux: None,
        labels: r.labels.clone(),
        annotations: r.annotations.clone(),
        runtime_handler: r.runtime_handler.clone(),
    }
}

pub fn record_to_sandbox(r: &oas_types::SandboxRecord) -> pb::PodSandbox {
    pb::PodSandbox {
        id: r.sandbox_id.clone(),
        metadata: Some(sandbox_metadata_from_domain(&r.metadata)),
        state: sandbox_state(r.state),
        created_at: secs_to_ns(r.created_at),
        labels: r.labels.clone(),
        annotations: r.annotations.clone(),
        runtime_handler: r.runtime_handler.clone(),
    }
}

pub fn sandbox_filter(f: Option<&pb::PodSandboxFilter>) -> oas_types::SandboxFilter {
    let Some(f) = f else {
        return oas_types::SandboxFilter::default();
    };
    oas_types::SandboxFilter {
        id: opt_string(&f.id),
        pod_uid: None,
        label_selector: f.label_selector.clone(),
        state: f.state.as_ref().map(|sv| match sv.state {
            x if x == pb::PodSandboxState::SandboxReady as i32 => oas_types::SandboxState::Ready,
            _ => oas_types::SandboxState::NotReady,
        }),
    }
}

// ---------------------------------------------------------------------------
// ContainerRecord → proto（ContainerStatus / Container）
// ---------------------------------------------------------------------------

pub fn record_to_container_status(r: &oas_types::ContainerRecord) -> pb::ContainerStatus {
    pb::ContainerStatus {
        id: r.container_id.clone(),
        metadata: Some(container_metadata_from_domain(&r.metadata)),
        state: container_state(r.state),
        created_at: secs_to_ns(r.created_at),
        started_at: r.started_at.map(secs_to_ns).unwrap_or(0),
        finished_at: r.finished_at.map(secs_to_ns).unwrap_or(0),
        exit_code: r.exit_code,
        image: Some(image_spec(&r.image)),
        image_ref: r.image.clone(),
        reason: exit_reason_str(r.reason).into(),
        message: r.message.clone(),
        labels: r.labels.clone(),
        annotations: r.annotations.clone(),
        mounts: Vec::new(),
        log_path: String::new(),
        resources: None,
        image_id: r.image.clone(),
        user: None,
        stop_signal: 0,
    }
}

pub fn record_to_container(r: &oas_types::ContainerRecord) -> pb::Container {
    pb::Container {
        id: r.container_id.clone(),
        pod_sandbox_id: r.sandbox_id.clone(),
        metadata: Some(container_metadata_from_domain(&r.metadata)),
        image: Some(image_spec(&r.image)),
        image_ref: r.image.clone(),
        state: container_state(r.state),
        created_at: secs_to_ns(r.created_at),
        labels: r.labels.clone(),
        annotations: r.annotations.clone(),
        image_id: r.image.clone(),
    }
}

pub fn container_filter(f: Option<&pb::ContainerFilter>) -> oas_types::ContainerFilter {
    let Some(f) = f else {
        return oas_types::ContainerFilter::default();
    };
    oas_types::ContainerFilter {
        id: opt_string(&f.id),
        sandbox_id: opt_string(&f.pod_sandbox_id),
        label_selector: f.label_selector.clone(),
        state: f.state.as_ref().map(|sv| match sv.state {
            x if x == pb::ContainerState::ContainerCreated as i32 => {
                oas_types::ContainerState::Created
            }
            x if x == pb::ContainerState::ContainerRunning as i32 => {
                oas_types::ContainerState::Running
            }
            x if x == pb::ContainerState::ContainerExited as i32 => {
                oas_types::ContainerState::Exited
            }
            _ => oas_types::ContainerState::Unknown,
        }),
    }
}

// ---------------------------------------------------------------------------
// ImageInfo → proto Image
// ---------------------------------------------------------------------------

pub fn image_info_to_image(i: &ImageInfo) -> pb::Image {
    pb::Image {
        id: i.id.clone(),
        repo_tags: i.repo_tags.clone(),
        repo_digests: i.repo_digests.clone(),
        size: i.size,
        uid: None,
        username: i.username.clone(),
        spec: Some(image_spec(&i.image_ref)),
        pinned: i.pinned,
    }
}

// ---------------------------------------------------------------------------
// Records →minimal stats
// ---------------------------------------------------------------------------

pub fn record_to_container_stats(r: &oas_types::ContainerRecord) -> pb::ContainerStats {
    let timestamp = r
        .started_at
        .or(Some(r.created_at))
        .map(secs_to_ns)
        .unwrap_or(1);

    pb::ContainerStats {
        attributes: Some(pb::ContainerAttributes {
            id: r.container_id.clone(),
            metadata: Some(container_metadata_from_domain(&r.metadata)),
            labels: r.labels.clone(),
            annotations: r.annotations.clone(),
        }),
        cpu: Some(pb::CpuUsage {
            timestamp,
            usage_core_nano_seconds: Some(pb::UInt64Value { value: 0 }),
            usage_nano_cores: Some(pb::UInt64Value { value: 0 }),
            psi: None,
        }),
        memory: Some(pb::MemoryUsage {
            timestamp,
            working_set_bytes: Some(pb::UInt64Value { value: 0 }),
            available_bytes: Some(pb::UInt64Value { value: 0 }),
            usage_bytes: Some(pb::UInt64Value { value: 0 }),
            rss_bytes: Some(pb::UInt64Value { value: 0 }),
            page_faults: Some(pb::UInt64Value { value: 0 }),
            major_page_faults: Some(pb::UInt64Value { value: 0 }),
            psi: None,
        }),
        writable_layer: Some(pb::FilesystemUsage {
            timestamp,
            fs_id: Some(pb::FilesystemIdentifier {
                mountpoint: "/".into(),
            }),
            used_bytes: Some(pb::UInt64Value { value: 0 }),
            inodes_used: Some(pb::UInt64Value { value: 0 }),
        }),
        swap: Some(pb::SwapUsage {
            timestamp,
            swap_available_bytes: Some(pb::UInt64Value { value: 0 }),
            swap_usage_bytes: Some(pb::UInt64Value { value: 0 }),
        }),
        io: Some(pb::IoUsage {
            timestamp,
            psi: None,
        }),
    }
}

pub fn records_to_pod_sandbox_stats(
    sandbox: &oas_types::SandboxRecord,
    containers: Vec<oas_types::ContainerRecord>,
) -> pb::PodSandboxStats {
    let timestamp = secs_to_ns(sandbox.created_at).max(1);

    pb::PodSandboxStats {
        attributes: Some(pb::PodSandboxAttributes {
            id: sandbox.sandbox_id.clone(),
            metadata: Some(sandbox_metadata_from_domain(&sandbox.metadata)),
            labels: sandbox.labels.clone(),
            annotations: sandbox.annotations.clone(),
        }),
        linux: Some(pb::LinuxPodSandboxStats {
            cpu: Some(pb::CpuUsage {
                timestamp,
                usage_core_nano_seconds: Some(pb::UInt64Value { value: 0 }),
                usage_nano_cores: Some(pb::UInt64Value { value: 0 }),
                psi: None,
            }),
            memory: Some(pb::MemoryUsage {
                timestamp,
                working_set_bytes: Some(pb::UInt64Value { value: 0 }),
                available_bytes: Some(pb::UInt64Value { value: 0 }),
                usage_bytes: Some(pb::UInt64Value { value: 0 }),
                rss_bytes: Some(pb::UInt64Value { value: 0 }),
                page_faults: Some(pb::UInt64Value { value: 0 }),
                major_page_faults: Some(pb::UInt64Value { value: 0 }),
                psi: None,
            }),
            network: Some(pb::NetworkUsage {
                timestamp,
                default_interface: Some(pb::NetworkInterfaceUsage {
                    name: "eth0".into(),
                    rx_bytes: Some(pb::UInt64Value { value: 0 }),
                    rx_errors: Some(pb::UInt64Value { value: 0 }),
                    tx_bytes: Some(pb::UInt64Value { value: 0 }),
                    tx_errors: Some(pb::UInt64Value { value: 0 }),
                }),
                interfaces: Vec::new(),
            }),
            process: Some(pb::ProcessUsage {
                timestamp,
                process_count: Some(pb::UInt64Value { value: 0 }),
            }),
            containers: containers.iter().map(record_to_container_stats).collect(),
            io: Some(pb::IoUsage {
                timestamp,
                psi: None,
            }),
        }),
        windows: None,
    }
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

fn opt_string(s: &str) -> Option<String> {
    if s.is_empty() {
        None
    } else {
        Some(s.to_string())
    }
}

/// 内部 record 以 Unix 秒存时间戳（§2.1），CRI proto 要求纳秒。emit 时换算。
pub fn secs_to_ns(s: i64) -> i64 {
    s.saturating_mul(1_000_000_000)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn metadata_roundtrip_preserved() {
        let m = pb::PodSandboxMetadata {
            name: "nginx".into(),
            uid: "uid-123".into(),
            namespace: "default".into(),
            attempt: 2,
        };
        let d = pod_metadata_to_domain(&m);
        assert_eq!(d.name, "nginx");
        assert_eq!(d.uid, "uid-123");
        assert_eq!(d.namespace, "default");
        assert_eq!(d.attempt, 2);
        let back = sandbox_metadata_from_domain(&d);
        assert_eq!(back.name, m.name);
        assert_eq!(back.uid, m.uid);
        assert_eq!(back.namespace, m.namespace);
        assert_eq!(back.attempt, m.attempt);
    }

    #[test]
    fn labels_annotations_pass_through() {
        let mut labels = HashMap::new();
        labels.insert("k".to_string(), "v".to_string());
        let mut ann = HashMap::new();
        ann.insert(ANNOT_TYPE.to_string(), "1".to_string());
        let cfg = pb::PodSandboxConfig {
            metadata: Some(pb::PodSandboxMetadata {
                name: "n".into(),
                uid: "u".into(),
                namespace: "ns".into(),
                attempt: 0,
            }),
            hostname: "h".into(),
            log_directory: "/log".into(),
            dns_config: None,
            port_mappings: Vec::new(),
            labels: labels.clone(),
            annotations: ann.clone(),
            linux: None,
            windows: None,
        };
        let req = sandbox_config_to_create_req(&cfg, "oas").unwrap();
        assert_eq!(req.labels, labels);
        assert_eq!(req.annotations, ann);
        assert_eq!(req.type_id, 1);
        assert_eq!(req.runtime_handler, "oas");
    }

    #[test]
    fn state_mapping() {
        assert_eq!(
            sandbox_state(oas_types::SandboxState::Ready),
            pb::PodSandboxState::SandboxReady as i32
        );
        assert_eq!(
            container_state(oas_types::ContainerState::Exited),
            pb::ContainerState::ContainerExited as i32
        );
    }

    #[test]
    fn missing_type_annotation_is_invalid_argument() {
        let cfg = pb::PodSandboxConfig {
            metadata: Some(pb::PodSandboxMetadata::default()),
            ..Default::default()
        };
        match sandbox_config_to_create_req(&cfg, "oas") {
            Err(OasError::InvalidArgument(_)) => {}
            other => panic!("expected InvalidArgument, got {other:?}"),
        }
    }

    #[test]
    fn runtime_handler_roundtrip_in_sandbox_status() {
        let rec = oas_types::SandboxRecord {
            sandbox_id: "sb-1".into(),
            pod_uid: "uid-1".into(),
            metadata: oas_types::SandboxMetadata {
                name: "n".into(),
                namespace: "ns".into(),
                uid: "uid-1".into(),
                attempt: 0,
            },
            labels: HashMap::new(),
            annotations: HashMap::new(),
            type_id: 0,
            netns_path: "/var/run/netns/oas-sb-1".into(),
            tap_name: "tapH0".into(),
            mac: "06:00:AC:10:00:02".into(),
            pod_ip: "10.244.0.2".into(),
            gateway: "10.244.0.254".into(),
            rw_layer_path: None,
            cloud_disk_dev: None,
            state: oas_types::SandboxState::Ready,
            created_at: 1,
            runtime_handler: "oas".into(),
            host_veth: String::new(),
        };
        let status = record_to_sandbox_status(&rec);
        assert_eq!(status.runtime_handler, "oas");
        assert_eq!(record_to_sandbox(&rec).runtime_handler, "oas");
    }
}
