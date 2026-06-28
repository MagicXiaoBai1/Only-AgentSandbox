//! `RuntimeService`（30 RPC）实现。真实 14 个委托 Manager，其余 16 个 unsupported。
//!
//! 每个真实方法 ≤20 行：convert → 调 manager → convert 回。幂等删除（Stop*/Remove*）
//! 走 `error::swallow_not_found` 把 NotFound 吞成 Ok（§3.1 红线）。

use std::collections::HashMap;
use std::sync::Arc;

use oas_manager::Manager;
use tonic::{Request, Response, Status};

use crate::convert;
use crate::error::{self, to_status};
use crate::runtime::v1 as pb;
use crate::runtime::v1::runtime_service_server::RuntimeService;
use crate::unsupported;

/// CRI `RuntimeService` 服务端。持 `Arc<dyn Manager>`，无自身状态。
#[derive(Clone)]
pub struct RuntimeSvc {
    mgr: Arc<dyn Manager>,
}

impl RuntimeSvc {
    pub fn new(mgr: Arc<dyn Manager>) -> Self {
        Self { mgr }
    }
}

#[tonic::async_trait]
impl RuntimeService for RuntimeSvc {
    // ---- 真实：握手 / 配置 ----

    async fn version(
        &self,
        req: Request<pb::VersionRequest>,
    ) -> Result<Response<pb::VersionResponse>, Status> {
        let v = self.mgr.version().await.map_err(to_status)?;
        Ok(Response::new(pb::VersionResponse {
            version: req.into_inner().version,
            runtime_name: v.runtime_name,
            runtime_version: v.runtime_version,
            runtime_api_version: v.runtime_api_version,
        }))
    }

    async fn status(
        &self,
        _req: Request<pb::StatusRequest>,
    ) -> Result<Response<pb::StatusResponse>, Status> {
        let s = self.mgr.status().await.map_err(to_status)?;
        let conditions = s
            .conditions
            .into_iter()
            .map(|c| pb::RuntimeCondition {
                r#type: c.r#type,
                status: c.status,
                reason: c.reason,
                message: c.message,
            })
            .collect();
        Ok(Response::new(pb::StatusResponse {
            status: Some(pb::RuntimeStatus { conditions }),
            info: HashMap::new(),
            runtime_handlers: Vec::new(),
            features: None,
        }))
    }

    async fn update_runtime_config(
        &self,
        req: Request<pb::UpdateRuntimeConfigRequest>,
    ) -> Result<Response<pb::UpdateRuntimeConfigResponse>, Status> {
        let r = req.into_inner();
        let pod_cidr = r
            .runtime_config
            .as_ref()
            .and_then(|rc| rc.network_config.as_ref())
            .map(|nc| nc.pod_cidr.clone())
            .filter(|s| !s.is_empty());
        self.mgr
            .update_runtime_config(pod_cidr.as_deref())
            .await
            .map_err(to_status)?;
        Ok(Response::new(pb::UpdateRuntimeConfigResponse::default()))
    }

    // ---- 真实：sandbox 生命周期 ----

    async fn run_pod_sandbox(
        &self,
        req: Request<pb::RunPodSandboxRequest>,
    ) -> Result<Response<pb::RunPodSandboxResponse>, Status> {
        let r = req.into_inner();
        let cfg = r
            .config
            .ok_or_else(|| Status::invalid_argument("missing pod sandbox config"))?;
        let create_req = convert::sandbox_config_to_create_req(&cfg).map_err(to_status)?;
        let id = self.mgr.run_sandbox(create_req).await.map_err(to_status)?;
        Ok(Response::new(pb::RunPodSandboxResponse {
            pod_sandbox_id: id,
        }))
    }

    async fn stop_pod_sandbox(
        &self,
        req: Request<pb::StopPodSandboxRequest>,
    ) -> Result<Response<pb::StopPodSandboxResponse>, Status> {
        error::swallow_not_found(
            self.mgr
                .stop_sandbox(&req.into_inner().pod_sandbox_id)
                .await,
        )?;
        Ok(Response::new(pb::StopPodSandboxResponse::default()))
    }

    async fn remove_pod_sandbox(
        &self,
        req: Request<pb::RemovePodSandboxRequest>,
    ) -> Result<Response<pb::RemovePodSandboxResponse>, Status> {
        error::swallow_not_found(
            self.mgr
                .remove_sandbox(&req.into_inner().pod_sandbox_id)
                .await,
        )?;
        Ok(Response::new(pb::RemovePodSandboxResponse::default()))
    }

    async fn pod_sandbox_status(
        &self,
        req: Request<pb::PodSandboxStatusRequest>,
    ) -> Result<Response<pb::PodSandboxStatusResponse>, Status> {
        let rec = self
            .mgr
            .sandbox_status(&req.into_inner().pod_sandbox_id)
            .await
            .map_err(to_status)?;
        Ok(Response::new(pb::PodSandboxStatusResponse {
            status: Some(convert::record_to_sandbox_status(&rec)),
            info: HashMap::new(),
            containers_statuses: Vec::new(),
            timestamp: 0,
        }))
    }

    async fn list_pod_sandbox(
        &self,
        req: Request<pb::ListPodSandboxRequest>,
    ) -> Result<Response<pb::ListPodSandboxResponse>, Status> {
        let filter = convert::sandbox_filter(req.into_inner().filter.as_ref());
        let recs = self.mgr.list_sandboxes(filter).await.map_err(to_status)?;
        let items = recs.iter().map(convert::record_to_sandbox).collect();
        Ok(Response::new(pb::ListPodSandboxResponse { items }))
    }

    // ---- 真实：container 生命周期 ----

    async fn create_container(
        &self,
        req: Request<pb::CreateContainerRequest>,
    ) -> Result<Response<pb::CreateContainerResponse>, Status> {
        let r = req.into_inner();
        let cfg = r
            .config
            .ok_or_else(|| Status::invalid_argument("missing container config"))?;
        let create_req =
            convert::container_config_to_create_req(&r.pod_sandbox_id, &cfg).map_err(to_status)?;
        let id = self
            .mgr
            .create_container(create_req)
            .await
            .map_err(to_status)?;
        Ok(Response::new(pb::CreateContainerResponse {
            container_id: id,
        }))
    }

    async fn start_container(
        &self,
        req: Request<pb::StartContainerRequest>,
    ) -> Result<Response<pb::StartContainerResponse>, Status> {
        self.mgr
            .start_container(&req.into_inner().container_id)
            .await
            .map_err(to_status)?;
        Ok(Response::new(pb::StartContainerResponse::default()))
    }

    async fn stop_container(
        &self,
        req: Request<pb::StopContainerRequest>,
    ) -> Result<Response<pb::StopContainerResponse>, Status> {
        let r = req.into_inner();
        error::swallow_not_found(self.mgr.stop_container(&r.container_id, r.timeout).await)?;
        Ok(Response::new(pb::StopContainerResponse::default()))
    }

    async fn remove_container(
        &self,
        req: Request<pb::RemoveContainerRequest>,
    ) -> Result<Response<pb::RemoveContainerResponse>, Status> {
        error::swallow_not_found(
            self.mgr
                .remove_container(&req.into_inner().container_id)
                .await,
        )?;
        Ok(Response::new(pb::RemoveContainerResponse::default()))
    }

    async fn container_status(
        &self,
        req: Request<pb::ContainerStatusRequest>,
    ) -> Result<Response<pb::ContainerStatusResponse>, Status> {
        let rec = self
            .mgr
            .container_status(&req.into_inner().container_id)
            .await
            .map_err(to_status)?;
        Ok(Response::new(pb::ContainerStatusResponse {
            status: Some(convert::record_to_container_status(&rec)),
            info: HashMap::new(),
        }))
    }

    async fn list_containers(
        &self,
        req: Request<pb::ListContainersRequest>,
    ) -> Result<Response<pb::ListContainersResponse>, Status> {
        let filter = convert::container_filter(req.into_inner().filter.as_ref());
        let recs = self.mgr.list_containers(filter).await.map_err(to_status)?;
        let containers = recs.iter().map(convert::record_to_container).collect();
        Ok(Response::new(pb::ListContainersResponse { containers }))
    }

    // ---- unsupported（MVP 非目标，§0）----

    async fn update_container_resources(
        &self,
        _req: Request<pb::UpdateContainerResourcesRequest>,
    ) -> Result<Response<pb::UpdateContainerResourcesResponse>, Status> {
        Err(unsupported::unimpl("UpdateContainerResources"))
    }

    async fn reopen_container_log(
        &self,
        _req: Request<pb::ReopenContainerLogRequest>,
    ) -> Result<Response<pb::ReopenContainerLogResponse>, Status> {
        Err(unsupported::unimpl("ReopenContainerLog"))
    }

    async fn exec_sync(
        &self,
        _req: Request<pb::ExecSyncRequest>,
    ) -> Result<Response<pb::ExecSyncResponse>, Status> {
        Err(unsupported::unimpl("ExecSync"))
    }

    async fn exec(
        &self,
        _req: Request<pb::ExecRequest>,
    ) -> Result<Response<pb::ExecResponse>, Status> {
        Err(unsupported::unimpl("Exec"))
    }

    async fn attach(
        &self,
        _req: Request<pb::AttachRequest>,
    ) -> Result<Response<pb::AttachResponse>, Status> {
        Err(unsupported::unimpl("Attach"))
    }

    async fn port_forward(
        &self,
        _req: Request<pb::PortForwardRequest>,
    ) -> Result<Response<pb::PortForwardResponse>, Status> {
        Err(unsupported::unimpl("PortForward"))
    }

    async fn container_stats(
        &self,
        _req: Request<pb::ContainerStatsRequest>,
    ) -> Result<Response<pb::ContainerStatsResponse>, Status> {
        Err(unsupported::unimpl("ContainerStats"))
    }

    async fn list_container_stats(
        &self,
        _req: Request<pb::ListContainerStatsRequest>,
    ) -> Result<Response<pb::ListContainerStatsResponse>, Status> {
        Err(unsupported::unimpl("ListContainerStats"))
    }

    async fn pod_sandbox_stats(
        &self,
        _req: Request<pb::PodSandboxStatsRequest>,
    ) -> Result<Response<pb::PodSandboxStatsResponse>, Status> {
        Err(unsupported::unimpl("PodSandboxStats"))
    }

    async fn list_pod_sandbox_stats(
        &self,
        _req: Request<pb::ListPodSandboxStatsRequest>,
    ) -> Result<Response<pb::ListPodSandboxStatsResponse>, Status> {
        Err(unsupported::unimpl("ListPodSandboxStats"))
    }

    async fn checkpoint_container(
        &self,
        _req: Request<pb::CheckpointContainerRequest>,
    ) -> Result<Response<pb::CheckpointContainerResponse>, Status> {
        Err(unsupported::unimpl("CheckpointContainer"))
    }

    type GetContainerEventsStream = tokio_stream::Empty<Result<pb::ContainerEventResponse, Status>>;

    async fn get_container_events(
        &self,
        _req: Request<pb::GetEventsRequest>,
    ) -> Result<Response<Self::GetContainerEventsStream>, Status> {
        Err(unsupported::unimpl("GetContainerEvents"))
    }

    async fn list_metric_descriptors(
        &self,
        _req: Request<pb::ListMetricDescriptorsRequest>,
    ) -> Result<Response<pb::ListMetricDescriptorsResponse>, Status> {
        Err(unsupported::unimpl("ListMetricDescriptors"))
    }

    async fn list_pod_sandbox_metrics(
        &self,
        _req: Request<pb::ListPodSandboxMetricsRequest>,
    ) -> Result<Response<pb::ListPodSandboxMetricsResponse>, Status> {
        Err(unsupported::unimpl("ListPodSandboxMetrics"))
    }

    async fn runtime_config(
        &self,
        _req: Request<pb::RuntimeConfigRequest>,
    ) -> Result<Response<pb::RuntimeConfigResponse>, Status> {
        Err(unsupported::unimpl("RuntimeConfig"))
    }

    async fn update_pod_sandbox_resources(
        &self,
        _req: Request<pb::UpdatePodSandboxResourcesRequest>,
    ) -> Result<Response<pb::UpdatePodSandboxResourcesResponse>, Status> {
        Err(unsupported::unimpl("UpdatePodSandboxResources"))
    }
}
