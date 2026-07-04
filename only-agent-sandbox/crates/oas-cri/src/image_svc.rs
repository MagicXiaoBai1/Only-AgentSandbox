//! `ImageService`（5 RPC）实现：白名单校验委托 Manager；`ImageFsInfo` 在 CRI 直出
//! “磁盘很空”假值（§3.1，防误驱逐）。

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
        _req: Request<pb::ListImagesRequest>,
    ) -> Result<Response<pb::ListImagesResponse>, Status> {
        let imgs = self.mgr.list_images().await.map_err(to_status)?;
        let images = imgs.iter().map(convert::image_info_to_image).collect();
        Ok(Response::new(pb::ListImagesResponse { images }))
    }

    async fn image_status(
        &self,
        req: Request<pb::ImageStatusRequest>,
    ) -> Result<Response<pb::ImageStatusResponse>, Status> {
        let image = req
            .into_inner()
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
    }

    async fn pull_image(
        &self,
        req: Request<pb::PullImageRequest>,
    ) -> Result<Response<pb::PullImageResponse>, Status> {
        let image = req
            .into_inner()
            .image
            .map(|s| s.image)
            .ok_or_else(|| Status::invalid_argument("missing image"))?;
        let image_ref = self.mgr.pull_image(&image).await.map_err(to_status)?;
        Ok(Response::new(pb::PullImageResponse { image_ref }))
    }

    async fn remove_image(
        &self,
        req: Request<pb::RemoveImageRequest>,
    ) -> Result<Response<pb::RemoveImageResponse>, Status> {
        let image = req
            .into_inner()
            .image
            .map(|s| s.image)
            .ok_or_else(|| Status::invalid_argument("missing image"))?;
        // 幂等：删不存在 = Ok。
        error::swallow_not_found(self.mgr.remove_image(&image).await)?;
        Ok(Response::new(pb::RemoveImageResponse::default()))
    }

    async fn image_fs_info(
        &self,
        _req: Request<pb::ImageFsInfoRequest>,
    ) -> Result<Response<pb::ImageFsInfoResponse>, Status> {
        // 固定返回“磁盘很空”假值（used < capacity），避免触发误驱逐（§3.1）。
        // mountpoint 必须是真实存在的路径，kubelet 会 statfs 取真实容量；用根fs
        // 容量充足，不会触发 DiskPressure。
        let fs = pb::FilesystemUsage {
            timestamp: 0,
            fs_id: Some(pb::FilesystemIdentifier {
                mountpoint: "/".into(),
            }),
            used_bytes: Some(pb::UInt64Value { value: 0 }),
            inodes_used: Some(pb::UInt64Value { value: 0 }),
        };
        Ok(Response::new(pb::ImageFsInfoResponse {
            image_filesystems: vec![fs.clone()],
            container_filesystems: vec![fs],
        }))
    }
}
