//! Sandbox 类型表（§3.2）：`type_id 0..=2` → vCPU/mem/rootfs/drive 拓扑 + 镜像白名单。
//!
//! 启动加载，不热更。

use crate::OasError;

/// 单个 sandbox 类型的配置。
#[derive(Debug, Clone)]
pub struct SandboxType {
    pub type_id: u8,
    pub vcpu: u32,
    pub mem_mib: u32,
    pub snapshot_path: String,
    pub rootfs_ro: String,
    pub has_rw_layer: bool,
    pub has_cloud_disk: bool,
    pub image_whitelist: Vec<String>,
}

/// 类型注册表（type_id 0..=2）。
#[derive(Debug, Clone)]
pub struct SandboxTypeTable {
    entries: [SandboxType; 3],
}

impl Default for SandboxTypeTable {
    fn default() -> Self {
        Self {
            entries: [
                SandboxType {
                    type_id: 0,
                    vcpu: 1,
                    mem_mib: 512,
                    snapshot_path: "base-0.snap".into(),
                    rootfs_ro: "base-0.ext4".into(),
                    has_rw_layer: false,
                    has_cloud_disk: false,
                    image_whitelist: vec!["img-a".into()],
                },
                SandboxType {
                    type_id: 1,
                    vcpu: 2,
                    mem_mib: 2048,
                    snapshot_path: "base-1.snap".into(),
                    rootfs_ro: "base-1.ext4".into(),
                    has_rw_layer: true,
                    has_cloud_disk: false,
                    image_whitelist: vec!["img-b".into()],
                },
                SandboxType {
                    type_id: 2,
                    vcpu: 4,
                    mem_mib: 8192,
                    snapshot_path: "base-2.snap".into(),
                    rootfs_ro: "base-2.ext4".into(),
                    has_rw_layer: true,
                    has_cloud_disk: true,
                    image_whitelist: vec!["img-c".into()],
                },
            ],
        }
    }
}

impl SandboxTypeTable {
    pub fn new() -> Self {
        Self::default()
    }

    /// 按 `type_id` 取配置；越界 → `InvalidArgument`。
    pub fn get(&self, type_id: u8) -> Result<&SandboxType, OasError> {
        self.entries.get(type_id as usize).ok_or_else(|| {
            OasError::InvalidArgument(format!("unknown type_id: {type_id}"))
        })
    }

    /// 全部白名单镜像（供 `list_images`）。
    pub fn all_images(&self) -> Vec<&str> {
        self.entries
            .iter()
            .flat_map(|t| t.image_whitelist.iter().map(String::as_str))
            .collect()
    }

    /// 镜像是否被任一 type 的白名单允许（按 base name 匹配，兼容 kubelet
    /// 把 `img-a` 规范化为 `docker.io/library/img-a:latest`）。
    pub fn image_allowed_any(&self, image: &str) -> bool {
        self.entries.iter().any(|t| t.image_allowed(image))
    }
}

/// 取镜像引用的 base name：去 digest 算法前缀（`sha256:`）、去 registry/path、去 tag。
/// `sha256:img-a:latest` → `img-a`；`docker.io/library/img-a:latest` → `img-a`；`img-a` → `img-a`。
/// kubelet 可能把 `ImageStatus` 返回的 `id`（`sha256:…`）当作 CreateContainer 的 image 引用回传，
/// 故须兼容剥离 `sha256:` 前缀。
pub fn image_base_name(image: &str) -> &str {
    let image = image
        .strip_prefix("sha256:")
        .or_else(|| image.strip_prefix("sha512:"))
        .unwrap_or(image);
    let after_slash = image.rsplit('/').next().unwrap_or(image);
    match after_slash.rfind(':') {
        Some(i) => &after_slash[..i],
        None => after_slash,
    }
}

impl SandboxType {
    /// 镜像是否在本 type 的白名单内（按 base name 匹配）。
    pub fn image_allowed(&self, image: &str) -> bool {
        let base = image_base_name(image);
        self.image_whitelist
            .iter()
            .any(|w| image_base_name(w) == base)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_types() {
        let t = SandboxTypeTable::default();
        assert_eq!(t.get(0).unwrap().vcpu, 1);
        assert_eq!(t.get(0).unwrap().mem_mib, 512);
        assert!(!t.get(0).unwrap().has_rw_layer);
        assert!(!t.get(0).unwrap().has_cloud_disk);
        assert!(t.get(0).unwrap().image_whitelist.contains(&"img-a".to_string()));

        assert!(t.get(1).unwrap().has_rw_layer);
        assert!(!t.get(1).unwrap().has_cloud_disk);

        assert!(t.get(2).unwrap().has_rw_layer);
        assert!(t.get(2).unwrap().has_cloud_disk);
        assert_eq!(t.get(2).unwrap().vcpu, 4);
    }

    #[test]
    fn unknown_type_rejected() {
        let t = SandboxTypeTable::default();
        assert!(matches!(t.get(3), Err(OasError::InvalidArgument(_))));
        assert!(matches!(t.get(255), Err(OasError::InvalidArgument(_))));
    }

    #[test]
    fn all_images_flattens() {
        let t = SandboxTypeTable::default();
        let imgs = t.all_images();
        assert_eq!(imgs, vec!["img-a", "img-b", "img-c"]);
    }
}
