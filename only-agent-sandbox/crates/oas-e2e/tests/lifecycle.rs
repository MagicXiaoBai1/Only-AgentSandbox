//! 端到端：真实 `OasManager`（+ MemoryStore + mock driver/net/storage）经 CRI UDS gRPC
//! 跑完整生命周期。验证 CRI proto 翻译、状态映射、幂等、错误码全链路。

mod common;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use common::TestEnv;
use oas_cri::runtime::v1::image_service_client::ImageServiceClient;
use oas_cri::runtime::v1::image_service_server::ImageServiceServer;
use oas_cri::runtime::v1::runtime_service_client::RuntimeServiceClient;
use oas_cri::runtime::v1::runtime_service_server::RuntimeServiceServer;
use oas_cri::runtime::v1::*;
use oas_cri::{ImageSvc, RuntimeSvc};
use oas_manager::Manager;
use tonic::transport::{Endpoint, Server};

static SOCK_SEQ: AtomicU64 = AtomicU64::new(0);

fn sock_path() -> String {
    let n = SOCK_SEQ.fetch_add(1, Ordering::Relaxed);
    format!("/tmp/oas-e2e-{}-{n}.sock", std::process::id())
}

async fn serve(path: &str, mgr: Arc<dyn Manager>) -> tokio::task::JoinHandle<()> {
    let rt = RuntimeSvc::new(mgr.clone());
    let img = ImageSvc::new(mgr);
    let uds = tokio::net::UnixListener::bind(path).unwrap();
    let incoming = async_stream::stream! {
        loop {
            match uds.accept().await {
                Ok((s, _)) => yield Ok::<_, std::io::Error>(s),
                Err(_) => continue,
            }
        }
    };
    tokio::spawn(async move {
        let _ = Server::builder()
            .add_service(RuntimeServiceServer::new(rt))
            .add_service(ImageServiceServer::new(img))
            .serve_with_incoming(incoming)
            .await;
    })
}

fn pod_cfg(uid: &str, type_id: u8) -> PodSandboxConfig {
    PodSandboxConfig {
        metadata: Some(PodSandboxMetadata {
            name: format!("pod-{uid}"),
            uid: uid.into(),
            namespace: "default".into(),
            attempt: 0,
        }),
        annotations: [("agent-sandbox/type".to_string(), type_id.to_string())]
            .into_iter()
            .collect(),
        ..Default::default()
    }
}

fn container_cfg(image: &str) -> ContainerConfig {
    ContainerConfig {
        metadata: Some(ContainerMetadata {
            name: "c0".into(),
            attempt: 0,
        }),
        image: Some(ImageSpec {
            image: image.into(),
            ..Default::default()
        }),
        command: vec!["sh".into()],
        ..Default::default()
    }
}

struct Ctx {
    rt: RuntimeServiceClient<tonic::transport::Channel>,
    img: ImageServiceClient<tonic::transport::Channel>,
    _handle: tokio::task::JoinHandle<()>,
    path: String,
}

async fn setup(env: &TestEnv) -> Ctx {
    let path = sock_path();
    let _ = std::fs::remove_file(&path);
    let mgr: Arc<dyn Manager> = env.mgr.clone();
    let handle = serve(&path, mgr).await;
    let channel = Endpoint::from_shared(format!("unix:{path}"))
        .unwrap()
        .connect()
        .await
        .unwrap();
    Ctx {
        rt: RuntimeServiceClient::new(channel.clone()),
        img: ImageServiceClient::new(channel),
        _handle: handle,
        path,
    }
}

impl Drop for Ctx {
    fn drop(&mut self) {
        self._handle.abort();
        let _ = std::fs::remove_file(&self.path);
    }
}

async fn run_pod(rt: &mut RuntimeServiceClient<tonic::transport::Channel>, uid: &str) -> String {
    rt.run_pod_sandbox(RunPodSandboxRequest {
        config: Some(pod_cfg(uid, 0)),
        runtime_handler: String::new(),
    })
    .await
    .unwrap()
    .into_inner()
    .pod_sandbox_id
}

async fn create_ct(
    rt: &mut RuntimeServiceClient<tonic::transport::Channel>,
    sb_id: &str,
    image: &str,
) -> String {
    rt.create_container(CreateContainerRequest {
        pod_sandbox_id: sb_id.into(),
        config: Some(container_cfg(image)),
        ..Default::default()
    })
    .await
    .unwrap()
    .into_inner()
    .container_id
}

async fn ct_state(
    rt: &mut RuntimeServiceClient<tonic::transport::Channel>,
    cid: &str,
) -> i32 {
    rt.container_status(ContainerStatusRequest {
        container_id: cid.into(),
        ..Default::default()
    })
    .await
    .unwrap()
    .into_inner()
    .status
    .unwrap()
    .state
}

#[tokio::test]
async fn e2e_full_lifecycle() {
    let env = TestEnv::new();
    let mut c = setup(&env).await;

    // RunPodSandbox → READY + pod_ip
    let sb_id = run_pod(&mut c.rt, "uid-e2e").await;
    assert!(!sb_id.is_empty());
    let status = c
        .rt
        .pod_sandbox_status(PodSandboxStatusRequest {
            pod_sandbox_id: sb_id.clone(),
            ..Default::default()
        })
        .await
        .unwrap()
        .into_inner()
        .status
        .unwrap();
    assert_eq!(status.state, PodSandboxState::SandboxReady as i32);
    assert!(!status.network.unwrap().ip.is_empty());

    // CreateContainer → CREATED → Start → RUNNING + started_at
    let cid = create_ct(&mut c.rt, &sb_id, "img-a").await;
    assert_eq!(ct_state(&mut c.rt, &cid).await, ContainerState::ContainerCreated as i32);
    c.rt
        .start_container(StartContainerRequest {
            container_id: cid.clone(),
        })
        .await
        .unwrap();
    let cs = c
        .rt
        .container_status(ContainerStatusRequest {
            container_id: cid.clone(),
            ..Default::default()
        })
        .await
        .unwrap()
        .into_inner()
        .status
        .unwrap();
    assert_eq!(cs.state, ContainerState::ContainerRunning as i32);
    assert!(cs.started_at > 0);

    // StopContainer → EXITED, exit_code=0, reason=Completed
    c.rt
        .stop_container(StopContainerRequest {
            container_id: cid.clone(),
            timeout: 10,
        })
        .await
        .unwrap();
    let cs = c
        .rt
        .container_status(ContainerStatusRequest {
            container_id: cid.clone(),
            ..Default::default()
        })
        .await
        .unwrap()
        .into_inner()
        .status
        .unwrap();
    assert_eq!(cs.state, ContainerState::ContainerExited as i32);
    assert_eq!(cs.exit_code, 0);
    assert_eq!(cs.reason, "Completed");

    // RemoveContainer → 之后 ContainerStatus = NotFound
    c.rt
        .remove_container(RemoveContainerRequest {
            container_id: cid.clone(),
        })
        .await
        .unwrap();
    let err = c
        .rt
        .container_status(ContainerStatusRequest {
            container_id: cid,
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::NotFound);

    // StopPodSandbox → NotReady
    c.rt
        .stop_pod_sandbox(StopPodSandboxRequest {
            pod_sandbox_id: sb_id.clone(),
        })
        .await
        .unwrap();
    let st = c
        .rt
        .pod_sandbox_status(PodSandboxStatusRequest {
            pod_sandbox_id: sb_id.clone(),
            ..Default::default()
        })
        .await
        .unwrap()
        .into_inner()
        .status
        .unwrap();
    assert_eq!(st.state, PodSandboxState::SandboxNotready as i32);

    // RemovePodSandbox → 之后 PodSandboxStatus = NotFound
    c.rt
        .remove_pod_sandbox(RemovePodSandboxRequest {
            pod_sandbox_id: sb_id.clone(),
        })
        .await
        .unwrap();
    let err = c
        .rt
        .pod_sandbox_status(PodSandboxStatusRequest {
            pod_sandbox_id: sb_id,
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::NotFound);
}

#[tokio::test]
async fn e2e_list_pod_sandbox_after_run() {
    let env = TestEnv::new();
    let mut c = setup(&env).await;
    run_pod(&mut c.rt, "uid-1").await;
    let items = c
        .rt
        .list_pod_sandbox(ListPodSandboxRequest { filter: None })
        .await
        .unwrap()
        .into_inner()
        .items;
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].state, PodSandboxState::SandboxReady as i32);
    assert_eq!(items[0].metadata.as_ref().unwrap().uid, "uid-1");
}

#[tokio::test]
async fn e2e_list_containers_filter_by_sandbox() {
    let env = TestEnv::new();
    let mut c = setup(&env).await;
    let sb_id = run_pod(&mut c.rt, "uid-1").await;
    create_ct(&mut c.rt, &sb_id, "img-a").await;
    create_ct(&mut c.rt, &sb_id, "img-a").await;
    let containers = c
        .rt
        .list_containers(ListContainersRequest {
            filter: Some(ContainerFilter {
                pod_sandbox_id: sb_id.clone(),
                ..Default::default()
            }),
        })
        .await
        .unwrap()
        .into_inner()
        .containers;
    assert_eq!(containers.len(), 2);
    assert!(containers.iter().all(|c| c.pod_sandbox_id == sb_id));
}

#[tokio::test]
async fn e2e_image_status_whitelist() {
    let env = TestEnv::new();
    let mut c = setup(&env).await;
    let hit = c
        .img
        .image_status(ImageStatusRequest {
            image: Some(ImageSpec {
                image: "img-a".into(),
                ..Default::default()
            }),
            ..Default::default()
        })
        .await
        .unwrap()
        .into_inner()
        .image;
    assert!(hit.is_some());

    let miss = c
        .img
        .image_status(ImageStatusRequest {
            image: Some(ImageSpec {
                image: "img-x".into(),
                ..Default::default()
            }),
            ..Default::default()
        })
        .await
        .unwrap()
        .into_inner()
        .image;
    assert!(miss.is_none());

    let imgs = c
        .img
        .list_images(ListImagesRequest::default())
        .await
        .unwrap()
        .into_inner()
        .images;
    assert_eq!(imgs.len(), 3);
}

#[tokio::test]
async fn e2e_create_container_image_not_whitelisted_returns_not_found() {
    let env = TestEnv::new();
    let mut c = setup(&env).await;
    let sb_id = run_pod(&mut c.rt, "uid-1").await;
    let err = c
        .rt
        .create_container(CreateContainerRequest {
            pod_sandbox_id: sb_id,
            config: Some(container_cfg("img-x")),
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::NotFound, "ImageNotInList → not_found");
}

#[tokio::test]
async fn e2e_run_sandbox_idempotent_by_uid() {
    let env = TestEnv::new();
    let mut c = setup(&env).await;
    let id1 = run_pod(&mut c.rt, "uid-1").await;
    let id2 = run_pod(&mut c.rt, "uid-1").await;
    assert_eq!(id1, id2, "same pod_uid → same sandbox_id, no second VM");
    assert_eq!(env.driver.create_count(), 1);
}

#[tokio::test]
async fn e2e_remove_pod_sandbox_idempotent() {
    let env = TestEnv::new();
    let mut c = setup(&env).await;
    let sb_id = run_pod(&mut c.rt, "uid-1").await;
    c.rt
        .stop_pod_sandbox(StopPodSandboxRequest {
            pod_sandbox_id: sb_id.clone(),
        })
        .await
        .unwrap();
    c.rt
        .remove_pod_sandbox(RemovePodSandboxRequest {
            pod_sandbox_id: sb_id.clone(),
        })
        .await
        .unwrap();
    // 第二次删 = Ok（幂等）。
    c.rt
        .remove_pod_sandbox(RemovePodSandboxRequest {
            pod_sandbox_id: sb_id,
        })
        .await
        .unwrap();
}

/// 协议层契约测试（前后端分离的"后端"侧）。
///
/// 在 driver/net/storage 全 mock 的前提下，按 kubelet 对单个沙箱的 CRI 调用序列
/// （启动握手 → RunPodSandbox → PLEG 稳态读 → StopPodSandbox → RemovePodSandbox）
/// 验证 API(CRI)+Manager+Store 三层能让 kubelet 在协议层"满意"：每步返回正确状态、
/// 稳态读零 driver 调用（红线③，PLEG 不压底层）、无驱逐触发、无无限重建。
/// 后端三层 OK 后再做 firecracker 等底层实现。
#[tokio::test]
async fn e2e_kubelet_protocol_sandbox_lifecycle() {
    let env = TestEnv::new();
    let mut c = setup(&env).await;

    // 1) kubelet 启动握手：Version + Status（RuntimeReady/NetworkReady=true 才认为 runtime 就绪）
    let v = c
        .rt
        .version(VersionRequest {
            version: "0.1.0".into(),
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(v.runtime_name, "only-agent-sandbox");
    assert_eq!(v.runtime_api_version, "v1");
    let conds = c
        .rt
        .status(StatusRequest::default())
        .await
        .unwrap()
        .into_inner()
        .status
        .unwrap()
        .conditions;
    assert!(conds.iter().any(|x| x.r#type == "RuntimeReady" && x.status));
    assert!(conds.iter().any(|x| x.r#type == "NetworkReady" && x.status));

    // 2) ImageFsInfo：kubelet 周期性查磁盘防驱逐 → 必须返回"很空"假值
    let fs = c
        .img
        .image_fs_info(ImageFsInfoRequest::default())
        .await
        .unwrap()
        .into_inner();
    assert!(!fs.image_filesystems.is_empty());

    // 3) RunPodSandbox（type=0 数字注解）→ READY + pod_ip
    let sb_id = run_pod(&mut c.rt, "uid-kubelet").await;
    let st = c
        .rt
        .pod_sandbox_status(PodSandboxStatusRequest {
            pod_sandbox_id: sb_id.clone(),
            ..Default::default()
        })
        .await
        .unwrap()
        .into_inner()
        .status
        .unwrap();
    assert_eq!(st.state, PodSandboxState::SandboxReady as i32);
    assert!(!st.network.unwrap().ip.is_empty());

    // 4) PLEG 稳态：kubelet 每秒 relist；多次 List + Status，状态恒定、id 不变。
    //    红线③：稳态读路径零 driver 调用（PLEG 不压 firecracker）。
    let drive_get_before = env.driver.get_count();
    let drive_list_before = env.driver.list_count();
    let create_before = env.driver.create_count();
    for _ in 0..3 {
        let items = c
            .rt
            .list_pod_sandbox(ListPodSandboxRequest { filter: None })
            .await
            .unwrap()
            .into_inner()
            .items;
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, sb_id);
        assert_eq!(items[0].state, PodSandboxState::SandboxReady as i32);
        let st = c
            .rt
            .pod_sandbox_status(PodSandboxStatusRequest {
                pod_sandbox_id: sb_id.clone(),
                ..Default::default()
            })
            .await
            .unwrap()
            .into_inner()
            .status
            .unwrap();
        assert_eq!(st.state, PodSandboxState::SandboxReady as i32);
    }
    assert_eq!(env.driver.get_count(), drive_get_before, "PLEG 读不得打 driver");
    assert_eq!(env.driver.list_count(), drive_list_before, "PLEG 读不得打 driver");
    assert_eq!(env.driver.create_count(), create_before, "稳态无重建");

    // 5) StopPodSandbox → NotReady（kubelet 仍会列出已停止 pod）
    c.rt
        .stop_pod_sandbox(StopPodSandboxRequest {
            pod_sandbox_id: sb_id.clone(),
        })
        .await
        .unwrap();
    let st = c
        .rt
        .pod_sandbox_status(PodSandboxStatusRequest {
            pod_sandbox_id: sb_id.clone(),
            ..Default::default()
        })
        .await
        .unwrap()
        .into_inner()
        .status
        .unwrap();
    assert_eq!(st.state, PodSandboxState::SandboxNotready as i32);
    let items = c
        .rt
        .list_pod_sandbox(ListPodSandboxRequest { filter: None })
        .await
        .unwrap()
        .into_inner()
        .items;
    assert_eq!(items.len(), 1, "stopped pod 仍在 list 中");

    // 6) RemovePodSandbox → 之后 Status=NotFound、List 空
    c.rt
        .remove_pod_sandbox(RemovePodSandboxRequest {
            pod_sandbox_id: sb_id.clone(),
        })
        .await
        .unwrap();
    let err = c
        .rt
        .pod_sandbox_status(PodSandboxStatusRequest {
            pod_sandbox_id: sb_id.clone(),
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::NotFound);
    let items = c
        .rt
        .list_pod_sandbox(ListPodSandboxRequest { filter: None })
        .await
        .unwrap()
        .into_inner()
        .items;
    assert!(items.is_empty(), "remove 后 list 为空");

    // 7) 编排确有发生：driver create/delete 各一次、net teardown 一次（mock 被正确调度）
    assert_eq!(env.driver.create_count(), 1);
    assert_eq!(env.driver.delete_count(), 1);
    assert_eq!(env.net.teardown_count(), 1);
}
