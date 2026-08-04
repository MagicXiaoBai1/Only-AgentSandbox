//! `RuntimeService`（30 RPC）实现。真实 14 个委托 Manager，其余 16 个 unsupported。
//!
//! 每个真实方法 ≤20 行：convert → 调 manager → convert 回。幂等删除（Stop*/Remove*）
//! 走 `error::swallow_not_found` 把 NotFound 吞成 Ok（§3.1 红线）。
//!
//! 所有 RPC 经 `cri_call!` 包裹：debug 级别下打印入参与返回值，便于联调排障。

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
        let req = req.into_inner();
        cri_call!("Version", &req, async move {
            let v = self.mgr.version().await.map_err(to_status)?;
            Ok(Response::new(pb::VersionResponse {
                version: req.version,
                runtime_name: v.runtime_name,
                runtime_version: v.runtime_version,
                runtime_api_version: v.runtime_api_version,
            }))
        })
    }

    async fn status(
        &self,
        req: Request<pb::StatusRequest>,
    ) -> Result<Response<pb::StatusResponse>, Status> {
        let req = req.into_inner();
        cri_call!("Status", &req, async move {
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
                runtime_handlers: vec![pb::RuntimeHandler {
                    name: "oas".into(),
                    features: Some(pb::RuntimeHandlerFeatures {
                        recursive_read_only_mounts: false,
                        user_namespaces: false,
                    }),
                }],
                features: None,
            }))
        })
    }

    async fn update_runtime_config(
        &self,
        req: Request<pb::UpdateRuntimeConfigRequest>,
    ) -> Result<Response<pb::UpdateRuntimeConfigResponse>, Status> {
        let req = req.into_inner();
        cri_call!("UpdateRuntimeConfig", &req, async move {
            let pod_cidr = req
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
        })
    }

    // ---- 真实：sandbox 生命周期 ----

    async fn run_pod_sandbox(
        &self,
        req: Request<pb::RunPodSandboxRequest>,
    ) -> Result<Response<pb::RunPodSandboxResponse>, Status> {
        let req = req.into_inner();
        cri_call!("RunPodSandbox", &req, async move {
            let handler = req.runtime_handler.trim();
            if !(handler.is_empty() || handler == "oas") {
                return Err(Status::invalid_argument(format!(
                    "unsupported runtime_handler: {handler}"
                )));
            }
            let cfg = req
                .config
                .ok_or_else(|| Status::invalid_argument("missing pod sandbox config"))?;
            let create_req =
                convert::sandbox_config_to_create_req(&cfg, handler).map_err(to_status)?;
            let id = self.mgr.run_sandbox(create_req).await.map_err(to_status)?;
            Ok(Response::new(pb::RunPodSandboxResponse {
                pod_sandbox_id: id,
            }))
        })
    }

    async fn stop_pod_sandbox(
        &self,
        req: Request<pb::StopPodSandboxRequest>,
    ) -> Result<Response<pb::StopPodSandboxResponse>, Status> {
        let req = req.into_inner();
        cri_call!("StopPodSandbox", &req, async move {
            error::swallow_not_found(self.mgr.stop_sandbox(&req.pod_sandbox_id).await)?;
            Ok(Response::new(pb::StopPodSandboxResponse::default()))
        })
    }

    async fn remove_pod_sandbox(
        &self,
        req: Request<pb::RemovePodSandboxRequest>,
    ) -> Result<Response<pb::RemovePodSandboxResponse>, Status> {
        let req = req.into_inner();
        cri_call!("RemovePodSandbox", &req, async move {
            error::swallow_not_found(self.mgr.remove_sandbox(&req.pod_sandbox_id).await)?;
            Ok(Response::new(pb::RemovePodSandboxResponse::default()))
        })
    }

    async fn pod_sandbox_status(
        &self,
        req: Request<pb::PodSandboxStatusRequest>,
    ) -> Result<Response<pb::PodSandboxStatusResponse>, Status> {
        let req = req.into_inner();
        cri_call!("PodSandboxStatus", &req, async move {
            let rec = self
                .mgr
                .sandbox_status(&req.pod_sandbox_id)
                .await
                .map_err(to_status)?;
            Ok(Response::new(pb::PodSandboxStatusResponse {
                status: Some(convert::record_to_sandbox_status(&rec)),
                info: HashMap::new(),
                containers_statuses: Vec::new(),
                timestamp: 0,
            }))
        })
    }

    async fn list_pod_sandbox(
        &self,
        req: Request<pb::ListPodSandboxRequest>,
    ) -> Result<Response<pb::ListPodSandboxResponse>, Status> {
        let req = req.into_inner();
        cri_call!("ListPodSandbox", &req, async move {
            let filter = convert::sandbox_filter(req.filter.as_ref());
            let recs = self.mgr.list_sandboxes(filter).await.map_err(to_status)?;
            let items = recs.iter().map(convert::record_to_sandbox).collect();
            Ok(Response::new(pb::ListPodSandboxResponse { items }))
        })
    }

    // ---- 真实：container 生命周期 ----

    async fn create_container(
        &self,
        req: Request<pb::CreateContainerRequest>,
    ) -> Result<Response<pb::CreateContainerResponse>, Status> {
        let req = req.into_inner();
        cri_call!("CreateContainer", &req, async move {
            let cfg = req
                .config
                .ok_or_else(|| Status::invalid_argument("missing container config"))?;
            let create_req =
                convert::container_config_to_create_req(&req.pod_sandbox_id, &cfg).map_err(to_status)?;
            let id = self
                .mgr
                .create_container(create_req)
                .await
                .map_err(to_status)?;
            Ok(Response::new(pb::CreateContainerResponse {
                container_id: id,
            }))
        })
    }

    async fn start_container(
        &self,
        req: Request<pb::StartContainerRequest>,
    ) -> Result<Response<pb::StartContainerResponse>, Status> {
        let req = req.into_inner();
        cri_call!("StartContainer", &req, async move {
            self.mgr
                .start_container(&req.container_id)
                .await
                .map_err(to_status)?;
            Ok(Response::new(pb::StartContainerResponse::default()))
        })
    }

    async fn stop_container(
        &self,
        req: Request<pb::StopContainerRequest>,
    ) -> Result<Response<pb::StopContainerResponse>, Status> {
        let req = req.into_inner();
        cri_call!("StopContainer", &req, async move {
            error::swallow_not_found(self.mgr.stop_container(&req.container_id, req.timeout).await)?;
            Ok(Response::new(pb::StopContainerResponse::default()))
        })
    }

    async fn remove_container(
        &self,
        req: Request<pb::RemoveContainerRequest>,
    ) -> Result<Response<pb::RemoveContainerResponse>, Status> {
        let req = req.into_inner();
        cri_call!("RemoveContainer", &req, async move {
            error::swallow_not_found(self.mgr.remove_container(&req.container_id).await)?;
            Ok(Response::new(pb::RemoveContainerResponse::default()))
        })
    }

    async fn container_status(
        &self,
        req: Request<pb::ContainerStatusRequest>,
    ) -> Result<Response<pb::ContainerStatusResponse>, Status> {
        let req = req.into_inner();
        cri_call!("ContainerStatus", &req, async move {
            let rec = self
                .mgr
                .container_status(&req.container_id)
                .await
                .map_err(to_status)?;
            Ok(Response::new(pb::ContainerStatusResponse {
                status: Some(convert::record_to_container_status(&rec)),
                info: HashMap::new(),
            }))
        })
    }

    async fn list_containers(
        &self,
        req: Request<pb::ListContainersRequest>,
    ) -> Result<Response<pb::ListContainersResponse>, Status> {
        let req = req.into_inner();
        cri_call!("ListContainers", &req, async move {
            let filter = convert::container_filter(req.filter.as_ref());
            let recs = self.mgr.list_containers(filter).await.map_err(to_status)?;
            let containers = recs.iter().map(convert::record_to_container).collect();
            Ok(Response::new(pb::ListContainersResponse { containers }))
        })
    }

    // ---- unsupported（MVP 非目标，§0）----
    // 仍经 cri_call! 打印调用与（错误）返回，便于现场确认被调到并被拒。

    async fn update_container_resources(
        &self,
        req: Request<pb::UpdateContainerResourcesRequest>,
    ) -> Result<Response<pb::UpdateContainerResourcesResponse>, Status> {
        let req = req.into_inner();
        cri_call!("UpdateContainerResources", &req, async move {
            Err(unsupported::unimpl("UpdateContainerResources"))
        })
    }

    async fn reopen_container_log(
        &self,
        req: Request<pb::ReopenContainerLogRequest>,
    ) -> Result<Response<pb::ReopenContainerLogResponse>, Status> {
        let req = req.into_inner();
        cri_call!("ReopenContainerLog", &req, async move {
            Err(unsupported::unimpl("ReopenContainerLog"))
        })
    }

    async fn exec_sync(
        &self,
        req: Request<pb::ExecSyncRequest>,
    ) -> Result<Response<pb::ExecSyncResponse>, Status> {
        let req = req.into_inner();
        cri_call!("ExecSync", &req, async move {
            Err(unsupported::unimpl("ExecSync"))
        })
    }

    async fn exec(
        &self,
        req: Request<pb::ExecRequest>,
    ) -> Result<Response<pb::ExecResponse>, Status> {
        let req = req.into_inner();
        cri_call!("Exec", &req, async move {
            Err(unsupported::unimpl("Exec"))
        })
    }

    async fn attach(
        &self,
        req: Request<pb::AttachRequest>,
    ) -> Result<Response<pb::AttachResponse>, Status> {
        let req = req.into_inner();
        cri_call!("Attach", &req, async move {
            Err(unsupported::unimpl("Attach"))
        })
    }

    async fn port_forward(
        &self,
        req: Request<pb::PortForwardRequest>,
    ) -> Result<Response<pb::PortForwardResponse>, Status> {
        let req = req.into_inner();
        cri_call!("PortForward", &req, async move {
            Err(unsupported::unimpl("PortForward"))
        })
    }

    async fn container_stats(
        &self,
        req: Request<pb::ContainerStatsRequest>,
    ) -> Result<Response<pb::ContainerStatsResponse>, Status> {
        let req = req.into_inner();
        cri_call!("ContainerStats", &req, async move {
            let rec = self
                .mgr
                .container_status(&req.container_id)
                .await
                .map_err(to_status)?;
            Ok(Response::new(pb::ContainerStatsResponse {
                stats: Some(convert::record_to_container_stats(&rec)),
            }))
        })
    }

    async fn list_container_stats(
        &self,
        req: Request<pb::ListContainerStatsRequest>,
    ) -> Result<Response<pb::ListContainerStatsResponse>, Status> {
        let req = req.into_inner();
        cri_call!("ListContainerStats", &req, async move {
            let filter = req.filter.as_ref().map(|f| pb::ContainerFilter {
                id: f.id.clone(),
                pod_sandbox_id: f.pod_sandbox_id.clone(),
                state: None,
                label_selector: f.label_selector.clone(),
            });
            let recs = self
                .mgr
                .list_containers(convert::container_filter(filter.as_ref()))
                .await
                .map_err(to_status)?;
            Ok(Response::new(pb::ListContainerStatsResponse {
                stats: recs.iter().map(convert::record_to_container_stats).collect(),
            }))
        })
    }

    async fn pod_sandbox_stats(
        &self,
        req: Request<pb::PodSandboxStatsRequest>,
    ) -> Result<Response<pb::PodSandboxStatsResponse>, Status> {
        let req = req.into_inner();
        cri_call!("PodSandboxStats", &req, async move {
            let sandbox = self
                .mgr
                .sandbox_status(&req.pod_sandbox_id)
                .await
                .map_err(to_status)?;
            let containers = self
                .mgr
                .list_containers(oas_types::ContainerFilter {
                    sandbox_id: Some(req.pod_sandbox_id),
                    ..Default::default()
                })
                .await
                .map_err(to_status)?;
            Ok(Response::new(pb::PodSandboxStatsResponse {
                stats: Some(convert::records_to_pod_sandbox_stats(&sandbox, containers)),
            }))
        })
    }

    async fn list_pod_sandbox_stats(
        &self,
        req: Request<pb::ListPodSandboxStatsRequest>,
    ) -> Result<Response<pb::ListPodSandboxStatsResponse>, Status> {
        let req = req.into_inner();
        cri_call!("ListPodSandboxStats", &req, async move {
            let filter = req.filter.as_ref().map(|f| pb::PodSandboxFilter {
                id: f.id.clone(),
                state: None,
                label_selector: f.label_selector.clone(),
            });
            let sandboxes = self
                .mgr
                .list_sandboxes(convert::sandbox_filter(filter.as_ref()))
                .await
                .map_err(to_status)?;
            let mut stats = Vec::with_capacity(sandboxes.len());
            for sandbox in sandboxes {
                let containers = self
                    .mgr
                    .list_containers(oas_types::ContainerFilter {
                        sandbox_id: Some(sandbox.sandbox_id.clone()),
                        ..Default::default()
                    })
                    .await
                    .map_err(to_status)?;
                stats.push(convert::records_to_pod_sandbox_stats(&sandbox, containers));
            }
            Ok(Response::new(pb::ListPodSandboxStatsResponse { stats }))
        })
    }

    async fn checkpoint_container(
        &self,
        req: Request<pb::CheckpointContainerRequest>,
    ) -> Result<Response<pb::CheckpointContainerResponse>, Status> {
        let req = req.into_inner();
        cri_call!("CheckpointContainer", &req, async move {
            Err(unsupported::unimpl("CheckpointContainer"))
        })
    }

    type GetContainerEventsStream = tokio_stream::Empty<Result<pb::ContainerEventResponse, Status>>;

    async fn get_container_events(
        &self,
        req: Request<pb::GetEventsRequest>,
    ) -> Result<Response<Self::GetContainerEventsStream>, Status> {
        // 返回类型含 stream，不走 cri_call!（避免要求 stream 实现 Debug）；手动打印入参与错误返回。
        let req = req.into_inner();
        tracing::debug!(target: "oas-cri", api = "GetContainerEvents", req = ?req, "CRI call");
        let err = unsupported::unimpl("GetContainerEvents");
        tracing::debug!(target: "oas-cri", api = "GetContainerEvents", err = %err, "CRI return");
        Err(err)
    }

    async fn list_metric_descriptors(
        &self,
        req: Request<pb::ListMetricDescriptorsRequest>,
    ) -> Result<Response<pb::ListMetricDescriptorsResponse>, Status> {
        let req = req.into_inner();
        cri_call!("ListMetricDescriptors", &req, async move {
            Err(unsupported::unimpl("ListMetricDescriptors"))
        })
    }

    async fn list_pod_sandbox_metrics(
        &self,
        req: Request<pb::ListPodSandboxMetricsRequest>,
    ) -> Result<Response<pb::ListPodSandboxMetricsResponse>, Status> {
        let req = req.into_inner();
        cri_call!("ListPodSandboxMetrics", &req, async move {
            Err(unsupported::unimpl("ListPodSandboxMetrics"))
        })
    }

    async fn runtime_config(
        &self,
        req: Request<pb::RuntimeConfigRequest>,
    ) -> Result<Response<pb::RuntimeConfigResponse>, Status> {
        let req = req.into_inner();
        cri_call!("RuntimeConfig", &req, async move {
            Err(unsupported::unimpl("RuntimeConfig"))
        })
    }

    async fn update_pod_sandbox_resources(
        &self,
        req: Request<pb::UpdatePodSandboxResourcesRequest>,
    ) -> Result<Response<pb::UpdatePodSandboxResourcesResponse>, Status> {
        let req = req.into_inner();
        cri_call!("UpdatePodSandboxResources", &req, async move {
            Err(unsupported::unimpl("UpdatePodSandboxResources"))
        })
    }
}
