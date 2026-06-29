//! 直接驱动 `OasManager`（不经 UDS）的状态机 / 幂等 / 回滚 / 红线测试。

mod common;

use std::sync::Arc;

use common::{container_req, sandbox_req, NeverReadyReadiness, TestEnv};
use oas_manager::{Manager, OasError};
use oas_store::Store;
use oas_types::{ContainerExitReason, ContainerState, SandboxState};

// ===== Sandbox FSM =========================================================

#[tokio::test]
async fn run_sandbox_happy() {
    let env = TestEnv::new();
    let id = env.mgr.run_sandbox(sandbox_req("uid1", 0)).await.unwrap();
    assert!(id.starts_with("sb-"));

    let rec = env.mgr.sandbox_status(&id).await.unwrap();
    assert_eq!(rec.state, SandboxState::Ready);
    assert_eq!(rec.pod_uid, "uid1");
    assert!(!rec.pod_ip.is_empty());
    assert!(rec.vm_id > 0);
    assert_eq!(rec.created_at, 1_700_000_000);
    assert_eq!(env.driver.create_count(), 1);
    assert_eq!(env.net.setup_count(), 1);
    assert_eq!(env.storage.provision_count(), 1);
}

#[tokio::test]
async fn run_sandbox_idempotent_by_uid() {
    let env = TestEnv::new();
    let id1 = env.mgr.run_sandbox(sandbox_req("uid1", 0)).await.unwrap();
    let id2 = env.mgr.run_sandbox(sandbox_req("uid1", 0)).await.unwrap();
    assert_eq!(id1, id2);
    assert_eq!(env.driver.create_count(), 1, "no second VM");
}

#[tokio::test]
async fn run_sandbox_notready_then_conflict() {
    // stop 后 sandbox 进入 NotReady；同 uid 再 run → Conflict（需先 remove）。
    let env = TestEnv::new();
    let id = env.mgr.run_sandbox(sandbox_req("uid1", 0)).await.unwrap();
    env.mgr.stop_sandbox(&id).await.unwrap();
    let err = env
        .mgr
        .run_sandbox(sandbox_req("uid1", 0))
        .await
        .unwrap_err();
    assert!(matches!(err, OasError::Conflict(_)));
}

#[tokio::test]
async fn run_sandbox_type2_missing_cloud_disk() {
    let env = TestEnv::new();
    let err = env
        .mgr
        .run_sandbox(sandbox_req("uid1", 2))
        .await
        .unwrap_err();
    assert!(matches!(err, OasError::InvalidArgument(_)));
    assert_eq!(env.driver.create_count(), 0);
    assert_eq!(env.net.setup_count(), 0);
}

#[tokio::test]
async fn run_sandbox_type0_with_cloud_disk() {
    let env = TestEnv::new();
    let mut req = sandbox_req("uid1", 0);
    req.cloud_disk_ref = Some("/dev/vdz".into());
    let err = env.mgr.run_sandbox(req).await.unwrap_err();
    assert!(matches!(err, OasError::InvalidArgument(_)));
}

#[tokio::test]
async fn run_sandbox_storage_fail_rolls_back_net() {
    let env = TestEnv::new();
    env.storage.fail_provision(true);
    let err = env
        .mgr
        .run_sandbox(sandbox_req("uid1", 0))
        .await
        .unwrap_err();
    assert!(matches!(err, OasError::Internal(_)));
    assert_eq!(env.net.teardown_count(), 1, "net rolled back");
    assert_eq!(env.driver.create_count(), 0, "create_vm not reached");
    assert_eq!(env.storage.provision_count(), 1);
}

#[tokio::test]
async fn run_sandbox_create_vm_fail_rolls_back() {
    let env = TestEnv::new();
    env.driver.fail_create(true);
    let err = env
        .mgr
        .run_sandbox(sandbox_req("uid1", 0))
        .await
        .unwrap_err();
    assert!(matches!(err, OasError::Unavailable(_)));
    assert_eq!(env.storage.cleanup_count(), 1, "storage rolled back");
    assert_eq!(env.net.teardown_count(), 1, "net rolled back");
    assert_eq!(env.driver.delete_count(), 0, "no vm to delete");
}

#[tokio::test]
async fn run_sandbox_readiness_timeout_rolls_back_all() {
    let env = TestEnv::with_readiness(Arc::new(NeverReadyReadiness));
    let err = env
        .mgr
        .run_sandbox(sandbox_req("uid1", 0))
        .await
        .unwrap_err();
    assert!(matches!(err, OasError::Unavailable(_)));
    assert_eq!(env.driver.create_count(), 1);
    assert_eq!(env.driver.delete_count(), 1, "orphan VM deleted");
    assert_eq!(env.storage.cleanup_count(), 1);
    assert_eq!(env.net.teardown_count(), 1);
    assert!(env
        .store
        .get_sandbox_by_uid("uid1")
        .unwrap()
        .is_none(), "no record persisted");
}

#[tokio::test]
async fn stop_sandbox_polls_to_stopped_then_notready() {
    let env = TestEnv::new();
    let id = env.mgr.run_sandbox(sandbox_req("uid1", 0)).await.unwrap();
    env.mgr.stop_sandbox(&id).await.unwrap();
    assert_eq!(env.driver.delete_count(), 1);
    assert!(env.driver.get_count() >= 1, "polled get_vm");
    assert_eq!(env.net.teardown_count(), 1, "IP released");
    let rec = env.mgr.sandbox_status(&id).await.unwrap();
    assert_eq!(rec.state, SandboxState::NotReady);
}

#[tokio::test]
async fn remove_sandbox_idempotent() {
    let env = TestEnv::new();
    let id = env.mgr.run_sandbox(sandbox_req("uid1", 0)).await.unwrap();
    env.mgr.stop_sandbox(&id).await.unwrap();
    env.mgr.remove_sandbox(&id).await.unwrap();
    // 第二次删 = Ok（幂等）。
    env.mgr.remove_sandbox(&id).await.unwrap();
    assert!(matches!(
        env.mgr.sandbox_status(&id).await,
        Err(OasError::NotFound(_))
    ));
    assert_eq!(env.storage.cleanup_count(), 1);
}

#[tokio::test]
async fn read_paths_never_call_driver() {
    // 红线 3：List* / *Status 只读 store。
    let env = TestEnv::new();
    let id = env.mgr.run_sandbox(sandbox_req("uid1", 0)).await.unwrap();
    let _ = env.mgr.sandbox_status(&id).await;
    let _ = env.mgr.list_sandboxes(Default::default()).await;
    assert_eq!(env.driver.get_count(), 0);
    assert_eq!(env.driver.list_count(), 0);
}

// ===== Container FSM =======================================================

async fn run_sandbox_type0(env: &TestEnv) -> String {
    env.mgr.run_sandbox(sandbox_req("uid1", 0)).await.unwrap()
}

#[tokio::test]
async fn create_container_happy() {
    let env = TestEnv::new();
    let sid = run_sandbox_type0(&env).await;
    let cid = env
        .mgr
        .create_container(container_req(&sid, "img-a"))
        .await
        .unwrap();
    assert!(cid.starts_with("ct-"));
    let c = env.mgr.container_status(&cid).await.unwrap();
    assert_eq!(c.state, ContainerState::Created);
    assert_eq!(c.image, "img-a");
    assert_eq!(c.exit_code, 0);
    assert_eq!(c.env, vec!["K=V".to_string()]);
}

#[tokio::test]
async fn create_container_image_not_in_whitelist() {
    let env = TestEnv::new();
    let sid = run_sandbox_type0(&env).await;
    let err = env
        .mgr
        .create_container(container_req(&sid, "img-x"))
        .await
        .unwrap_err();
    assert!(matches!(err, OasError::ImageNotInList(_)));
}

#[tokio::test]
async fn create_container_memory_exceeds_type() {
    let env = TestEnv::new();
    let sid = run_sandbox_type0(&env).await; // type0 = 512M
    let mut r = container_req(&sid, "img-a");
    r.resources.memory_limit_bytes = Some(600 * 1_048_576);
    let err = env.mgr.create_container(r).await.unwrap_err();
    assert!(matches!(err, OasError::TypeMismatch(_)));
}

#[tokio::test]
async fn create_container_sandbox_missing() {
    let env = TestEnv::new();
    let err = env
        .mgr
        .create_container(container_req("nope", "img-a"))
        .await
        .unwrap_err();
    assert!(matches!(err, OasError::NotFound(_)));
}

#[tokio::test]
async fn start_container_created_to_running() {
    let env = TestEnv::new();
    let sid = run_sandbox_type0(&env).await;
    let cid = env
        .mgr
        .create_container(container_req(&sid, "img-a"))
        .await
        .unwrap();
    env.mgr.start_container(&cid).await.unwrap();
    let c = env.mgr.container_status(&cid).await.unwrap();
    assert_eq!(c.state, ContainerState::Running);
    assert_eq!(c.started_at, Some(1_700_000_000));
}

#[tokio::test]
async fn start_container_running_idempotent() {
    let env = TestEnv::new();
    let sid = run_sandbox_type0(&env).await;
    let cid = env
        .mgr
        .create_container(container_req(&sid, "img-a"))
        .await
        .unwrap();
    env.mgr.start_container(&cid).await.unwrap();
    // 第二次 start（已 Running）= Ok（幂等）。
    env.mgr.start_container(&cid).await.unwrap();
    assert_eq!(
        env.mgr.container_status(&cid).await.unwrap().state,
        ContainerState::Running
    );
}

#[tokio::test]
async fn start_container_exited_conflict() {
    let env = TestEnv::new();
    let sid = run_sandbox_type0(&env).await;
    let cid = env
        .mgr
        .create_container(container_req(&sid, "img-a"))
        .await
        .unwrap();
    env.mgr.stop_container(&cid, 10).await.unwrap(); // → Exited
    let err = env.mgr.start_container(&cid).await.unwrap_err();
    assert!(matches!(err, OasError::Conflict(_)));
}

#[tokio::test]
async fn stop_container_created_to_exited() {
    let env = TestEnv::new();
    let sid = run_sandbox_type0(&env).await;
    let cid = env
        .mgr
        .create_container(container_req(&sid, "img-a"))
        .await
        .unwrap();
    env.mgr.stop_container(&cid, 10).await.unwrap();
    let c = env.mgr.container_status(&cid).await.unwrap();
    assert_eq!(c.state, ContainerState::Exited);
    assert_eq!(c.exit_code, 0);
    assert_eq!(c.reason, Some(ContainerExitReason::Completed));
    assert!(c.finished_at.is_some());
}

#[tokio::test]
async fn stop_container_running_to_exited() {
    let env = TestEnv::new();
    let sid = run_sandbox_type0(&env).await;
    let cid = env
        .mgr
        .create_container(container_req(&sid, "img-a"))
        .await
        .unwrap();
    env.mgr.start_container(&cid).await.unwrap();
    env.mgr.stop_container(&cid, 10).await.unwrap();
    let c = env.mgr.container_status(&cid).await.unwrap();
    assert_eq!(c.state, ContainerState::Exited);
    assert_eq!(c.exit_code, 0);
}

#[tokio::test]
async fn stop_container_idempotent_on_exited() {
    let env = TestEnv::new();
    let sid = run_sandbox_type0(&env).await;
    let cid = env
        .mgr
        .create_container(container_req(&sid, "img-a"))
        .await
        .unwrap();
    env.mgr.stop_container(&cid, 10).await.unwrap();
    env.mgr.stop_container(&cid, 10).await.unwrap(); // 幂等
    assert_eq!(
        env.mgr.container_status(&cid).await.unwrap().state,
        ContainerState::Exited
    );
}

#[tokio::test]
async fn stop_container_missing_ok() {
    let env = TestEnv::new();
    env.mgr.stop_container("nope", 10).await.unwrap(); // 幂等
}

#[tokio::test]
async fn remove_container_idempotent() {
    let env = TestEnv::new();
    let sid = run_sandbox_type0(&env).await;
    let cid = env
        .mgr
        .create_container(container_req(&sid, "img-a"))
        .await
        .unwrap();
    env.mgr.remove_container(&cid).await.unwrap();
    env.mgr.remove_container(&cid).await.unwrap(); // 幂等
    assert!(matches!(
        env.mgr.container_status(&cid).await,
        Err(OasError::NotFound(_))
    ));
}

#[tokio::test]
async fn list_containers_by_sandbox() {
    let env = TestEnv::new();
    let sid = run_sandbox_type0(&env).await;
    let c1 = env
        .mgr
        .create_container(container_req(&sid, "img-a"))
        .await
        .unwrap();
    let c2 = env
        .mgr
        .create_container(container_req(&sid, "img-a"))
        .await
        .unwrap();
    let filter = oas_types::ContainerFilter {
        sandbox_id: Some(sid.clone()),
        ..Default::default()
    };
    let list = env.mgr.list_containers(filter).await.unwrap();
    assert_eq!(list.len(), 2);
    assert!(list.iter().any(|c| c.container_id == c1));
    assert!(list.iter().any(|c| c.container_id == c2));
}

#[tokio::test]
async fn container_status_never_calls_driver() {
    let env = TestEnv::new();
    let sid = run_sandbox_type0(&env).await;
    let cid = env
        .mgr
        .create_container(container_req(&sid, "img-a"))
        .await
        .unwrap();
    let _ = env.mgr.container_status(&cid).await;
    let _ = env.mgr.list_containers(Default::default()).await;
    assert_eq!(env.driver.get_count(), 0);
}

// ===== image_* + version/status ===========================================

#[tokio::test]
async fn image_status_hit_and_miss() {
    let env = TestEnv::new();
    assert!(env.mgr.image_status("img-a").await.unwrap().is_some());
    assert!(env.mgr.image_status("img-x").await.unwrap().is_none());
}

#[tokio::test]
async fn pull_image_whitelist_and_reject() {
    let env = TestEnv::new();
    assert_eq!(env.mgr.pull_image("img-b").await.unwrap(), "img-b");
    let err = env.mgr.pull_image("img-x").await.unwrap_err();
    assert!(matches!(err, OasError::ImageNotInList(_)));
}

#[tokio::test]
async fn list_images_flattens_whitelist() {
    let env = TestEnv::new();
    let imgs = env.mgr.list_images().await.unwrap();
    assert_eq!(imgs.len(), 3);
    let refs: Vec<&str> = imgs.iter().map(|i| i.image_ref.as_str()).collect();
    assert!(refs.contains(&"img-a"));
    assert!(refs.contains(&"img-b"));
    assert!(refs.contains(&"img-c"));
}

#[tokio::test]
async fn remove_image_noop() {
    let env = TestEnv::new();
    env.mgr.remove_image("img-a").await.unwrap();
}

#[tokio::test]
async fn version_and_status_fixed() {
    let env = TestEnv::new();
    let v = env.mgr.version().await.unwrap();
    assert_eq!(v.runtime_name, "only-agent-sandbox");
    assert_eq!(v.runtime_api_version, "v1");
    let s = env.mgr.status().await.unwrap();
    assert_eq!(s.conditions.len(), 2);
    assert!(s.conditions.iter().all(|c| c.status));
}
