//! `ImageService`（5 RPC）实现：白名单校验委托 Manager；`ImageFsInfo` 在 CRI 直出
//! “磁盘很空”假值（§3.1，防误驱逐）。
//!
//! 所有 RPC 经 `cri_call!` 包裹：debug 级别下打印入参与返回值，便于联调排障。

use std::sync::Arc;

use oas_manager::Manager;
use tonic::{Request, Response, Status};

use crate::convert;
use crate::error::{self, to_status};
use crate::runtime::v1 as pb;
use crate::runtime::v1::image_service_server::ImageService;

/// CRI `ImageService` 服务端。持 `Arc<dyn Manager>`。
#[derive(Clone)]
pub struct ImageSvc {
    mgr: Arc<dyn Manager>,
}

impl ImageSvc {
    pub fn new(mgr: Arc<dyn Manager>) -> Self {
        Self { mgr }
    }
}

#[tonic::async_trait]
impl ImageService for ImageSvc {
    async fn list_images(
        &self,
        req: Request<pb::ListImagesRequest>,
    ) -> Result<Response<pb::ListImagesResponse>, Status> {
        let req = req.into_inner();
        cri_call!("ListImages", &req, async move {
            let imgs = self.mgr.list_images().await.map_err(to_status)?;
            let images = imgs.iter().map(convert::image_info_to_image).collect();
            Ok(Response::new(pb::ListImagesResponse { images }))
        })
    }

    async fn image_status(
        &self,
        req: Request<pb::ImageStatusRequest>,
    ) -> Result<Response<pb::ImageStatusResponse>, Status> {
        let req = req.into_inner();
        cri_call!("ImageStatus", &req, async move {
            let image = req
                .image
                .map(|s| s.image)
                .ok_or_else(|| Status::invalid_argument("missing image"))?;
            match self.mgr.image_status(&image).await.map_err(to_status)? {
                Some(info) => Ok(Response::new(pb::ImageStatusResponse {
                    image: Some(convert::image_info_to_image(&info)),
                    info: Default::default(),
                })),
                None => Ok(Response::new(pb::ImageStatusResponse {
                    image: None,
                    info: Default::default(),
                })),
            }
        })
    }

    async fn pull_image(
        &self,
        req: Request<pb::PullImageRequest>,
    ) -> Result<Response<pb::PullImageResponse>, Status> {
        let req = req.into_inner();
        cri_call!("PullImage", &req, async move {
            let image = req
                .image
                .map(|s| s.image)
                .ok_or_else(|| Status::invalid_argument("missing image"))?;
            let image_ref = self.mgr.pull_image(&image).await.map_err(to_status)?;
            Ok(Response::new(pb::PullImageResponse { image_ref }))
        })
    }

    async fn remove_image(
        &self,
        req: Request<pb::RemoveImageRequest>,
    ) -> Result<Response<pb::RemoveImageResponse>, Status> {
        let req = req.into_inner();
        cri_call!("RemoveImage", &req, async move {
            let image = req
                .image
                .map(|s| s.image)
                .ok_or_else(|| Status::invalid_argument("missing image"))?;
            // 幂等：删不存在 = Ok。
            error::swallow_not_found(self.mgr.remove_image(&image).await)?;
            Ok(Response::new(pb::RemoveImageResponse::default()))
        })
    }

    async fn image_fs_info(
        &self,
        req: Request<pb::ImageFsInfoRequest>,
    ) -> Result<Response<pb::ImageFsInfoResponse>, Status> {
        let req = req.into_inner();

        cri_call!("ImageFsInfo", &req, async move {
            // mountpoint 必须是真实存在的路径，kubelet 会 statfs 取真实容量。
            // 只声明 image filesystems；container_filesystems 为空，沿用 CRI 默认语义。
            // 避免 kubelet 对同一个假容器文件系统做额外容量推断。
            let fs = pb::FilesystemUsage {
                timestamp: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos().min(i64::MAX as u128) as i64)
                    .unwrap_or(1),
                fs_id: Some(pb::FilesystemIdentifier {
                    mountpoint: "/".into(),
                }),
                used_bytes: Some(pb::UInt64Value { value: 0 }),
                inodes_used: Some(pb::UInt64Value { value: 0 }),
            };

            Ok(Response::new(pb::ImageFsInfoResponse {
                image_filesystems: vec![fs],
                container_filesystems: Vec::new(),
            }))
        })
    }
}
