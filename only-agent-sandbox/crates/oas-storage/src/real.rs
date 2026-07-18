//! 真 `StorageManager`：per-VM 可写 ext4 文件制备（`truncate` + `mkfs.ext4`）。
//!
//! cloud_disk（块设备）MVP 不恢复（driver 返回 Unsupported）；storage 仅记录 `cloud_disk_ref`。

use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;

use async_trait::async_trait;
use oas_config::Config;

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
                run(
                    "truncate",
                    &["-s", &format!("{}M", cfg.rw_size_mib), &p.to_string_lossy()],
                )?;
                run("mkfs.ext4", &["-F", &p.to_string_lossy()])?;
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
