//! 真 `StorageManager`：per-VM 可写 ext4 文件制备（`truncate` + `mkfs.ext4`）。
//!
//! cloud_disk（块设备）MVP 不恢复（driver 返回 Unsupported）；storage 仅记录 `cloud_disk_ref`。

use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;

use async_trait::async_trait;
use oas_config::{Config, copy_file_with_reflink};

use crate::{DiskConfig, StorageError, StorageManager};

pub struct RealStorageManager {
    cfg: Arc<Config>,
}

impl RealStorageManager {
    pub fn new(cfg: Arc<Config>) -> Self {
        Self { cfg }
    }
}

fn run(cmd: &str, args: &[&str]) -> Result<(), StorageError> {
    let out = Command::new(cmd)
        .args(args)
        .output()
        .map_err(|e| StorageError::Other(format!("{cmd} {}: {e}", args.join(" "))))?;
    if !out.status.success() {
        return Err(StorageError::Other(format!(
            "{cmd} {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr)
        )));
    }
    Ok(())
}

#[async_trait]
impl StorageManager for RealStorageManager {
    async fn provision(
        &self,
        sandbox_id: &str,
        type_id: u8,
        cloud_disk_ref: Option<&str>,
    ) -> Result<DiskConfig, StorageError> {
        let cfg = self.cfg.clone();
        let sid = sandbox_id.to_string();
        let cdr = cloud_disk_ref.map(|s| s.to_string());
        let ty_has_rw = cfg
            .get_type(type_id)
            .map(|t| t.has_rw_layer)
            .unwrap_or(false);

        tokio::task::spawn_blocking(move || {
            let rw_layer_path = if ty_has_rw {
                let p: PathBuf = cfg.rw_base_dir.join(format!("{sid}.ext4"));
                std::fs::create_dir_all(&cfg.rw_base_dir)?;
                if let Some(template) = cfg.rw_template_path.as_ref() {
                    if !template.is_file() {
                        return Err(StorageError::Other(format!(
                            "rw CoW template does not exist: {} (run tools/prepare_rw_template.sh first)",
                            template.display()
                        )));
                    }
                    let method = copy_file_with_reflink(
                        template,
                        &p,
                        cfg.reflink_enabled,
                        cfg.reflink_required,
                    )
                    .map_err(|e| {
                        StorageError::Other(format!(
                            "rw CoW template reflink {} -> {}: {e}",
                            template.display(),
                            p.display()
                        ))
                    })?;
                    tracing::debug!(
                        sandbox_id = %sid,
                        template = %template.display(),
                        destination = %p.display(),
                        ?method,
                        "provisioned reflink-backed ext4 rw layer"
                    );
                    // reflink 会复制 ext4 superblock，必须为每个实例生成新 UUID，
                    // 否则 guest 内按 UUID 发现磁盘时可能把两个实例误认为同一设备。
                    run("tune2fs", &["-U", "random", &p.to_string_lossy()])?;
                } else if cfg.reflink_required {
                    return Err(StorageError::Other(
                        "rw_template_path is required when reflink_required=true".into(),
                    ));
                } else {
                    // 仅作为显式开发回退；生产默认 reflink_required=true，不会走这里。
                    run(
                        "truncate",
                        &["-s", &format!("{}M", cfg.rw_size_mib), &p.to_string_lossy()],
                    )?;
                    run("mkfs.ext4", &["-F", &p.to_string_lossy()])?;
                }
                Some(p.to_string_lossy().into_owned())
            } else {
                None
            };
            Ok(DiskConfig {
                rw_layer_path,
                cloud_disk_dev: cdr,
            })
        })
        .await
        .map_err(|e| StorageError::Other(format!("blocking join: {e}")))?
    }

    async fn cleanup(&self, disk: &DiskConfig) -> Result<(), StorageError> {
        if let Some(p) = &disk.rw_layer_path {
            let _ = std::fs::remove_file(p);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::RealStorageManager;
    use crate::StorageManager;
    use oas_config::{Config, CopyMethod, copy_file_with_reflink};
    use std::fs;
    use std::path::PathBuf;
    use std::process::Command;
    use std::sync::Arc;

    #[tokio::test]
    async fn provision_uses_reflink_template_when_filesystem_supports_it() {
        if Command::new("mkfs.ext4").arg("-V").output().is_err()
            || Command::new("tune2fs").arg("-V").output().is_err()
        {
            return;
        }
        let root_base = std::env::var_os("OAS_TEST_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| std::env::current_dir().unwrap());
        let root = root_base.join(format!("oas-storage-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let template = root.join("rw-template.ext4");
        let probe = root.join("probe.ext4");
        let instance = root.join("rw").join("sandbox.ext4");
        Command::new("truncate")
            .args(["-s", "32M", &template.to_string_lossy()])
            .status()
            .unwrap();
        assert!(
            Command::new("mkfs.ext4")
                .args(["-F", &template.to_string_lossy()])
                .status()
                .unwrap()
                .success()
        );
        let method = copy_file_with_reflink(&template, &probe, true, false).unwrap();
        if method != CopyMethod::Reflink {
            let _ = fs::remove_dir_all(&root);
            return;
        }
        let mut cfg = Config::default();
        cfg.rw_base_dir = root.join("rw");
        cfg.rw_template_path = Some(template);
        cfg.reflink_enabled = true;
        cfg.reflink_required = true;
        let manager = RealStorageManager::new(Arc::new(cfg));
        let disk = manager.provision("sandbox", 1, None).await.unwrap();
        assert_eq!(
            disk.rw_layer_path.as_deref(),
            Some(instance.to_str().unwrap())
        );
        assert!(instance.is_file());
        manager.cleanup(&disk).await.unwrap();
        let _ = fs::remove_file(probe);
        let _ = fs::remove_dir_all(root);
    }
}
