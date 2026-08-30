//! Store 一致性测试套件：对任意 `Store` 实现跑同一组用例。
//!
//! `memory.rs` / `redb.rs` 各自带一个 `#[test]` 调 `run_store_tests`。

use std::collections::HashMap;

use oas_store::{Store, StoreError};
use oas_types::{
    ContainerFilter, ContainerRecord, ContainerState, IpLease, SandboxFilter, SandboxMetadata,
    SandboxRecord, SandboxState,
};

// ---- 记录构造助手 ----------------------------------------------------------

fn sb(id: &str, uid: &str, state: SandboxState) -> SandboxRecord {
    SandboxRecord {
        sandbox_id: id.into(),
        pod_uid: uid.into(),
        metadata: SandboxMetadata {
            name: format!("n-{id}"),
            namespace: "ns".into(),
            uid: uid.into(),
            attempt: 0,
        },
        labels: HashMap::new(),
        annotations: HashMap::new(),
        type_id: 0,
        netns_path: format!("/ns/{id}"),
        tap_name: "tap0".into(),
        mac: "aa".into(),
        pod_ip: "10.0.0.1".into(),
        gateway: "10.0.0.1".into(),
        rw_layer_path: None,
        cloud_disk_dev: None,
        state,
        created_at: 100,
        host_veth: String::new(),
    }
}

fn ct(id: &str, sid: &str, state: ContainerState) -> ContainerRecord {
    ContainerRecord {
        container_id: id.into(),
        sandbox_id: sid.into(),
        metadata: oas_types::ContainerMetadata {
            name: format!("c-{id}"),
            attempt: 0,
        },
        image: "img-a".into(),
        command: vec!["sh".into()],
        args: vec!["-c".into(), "true".into()],
        env: vec!["K=V".into()],
        mounts: vec![],
        resources: oas_types::LinuxResources::default(),
        state,
        created_at: 100,
        started_at: None,
        finished_at: None,
        exit_code: 0,
        reason: None,
        message: String::new(),
        labels: HashMap::new(),
        annotations: HashMap::new(),
    }
}

// ---- 套件入口 --------------------------------------------------------------

pub fn run_store_tests<S: Store>(make: impl Fn() -> S) {
    sandbox_crud(&make);
    sandbox_get_by_uid(&make);
    sandbox_list_filters(&make);
    sandbox_delete_idempotent(&make);
    container_crud(&make);
    container_list_filters(&make);
    container_delete_idempotent(&make);
    ipam(&make);
    transaction_commit_and_rollback(&make);
    metadata_passthrough(&make);
}

// ---- sandbox CRUD ----------------------------------------------------------

fn sandbox_crud<S: Store>(make: &impl Fn() -> S) {
    let s = make();
    let r = sb("sb1", "uid1", SandboxState::Ready);
    s.put_sandbox(&r).unwrap();
    let got = s.get_sandbox("sb1").unwrap();
    assert_eq!(got.sandbox_id, "sb1");
    assert_eq!(got.pod_uid, "uid1");
    assert_eq!(got.state, SandboxState::Ready);

    // 覆盖写
    let mut r2 = r.clone();
    r2.tap_name = "tap9".into();
    s.put_sandbox(&r2).unwrap();
    assert_eq!(s.get_sandbox("sb1").unwrap().tap_name, "tap9");

    // 缺失 → NotFound
    match s.get_sandbox("nope") {
        Err(StoreError::NotFound(_)) => {}
        other => panic!("expected NotFound, got {other:?}"),
    }
}

fn sandbox_get_by_uid<S: Store>(make: &impl Fn() -> S) {
    let s = make();
    s.put_sandbox(&sb("sb1", "uid1", SandboxState::Ready))
        .unwrap();
    assert_eq!(
        s.get_sandbox_by_uid("uid1").unwrap().unwrap().sandbox_id,
        "sb1"
    );
    assert!(s.get_sandbox_by_uid("uid2").unwrap().is_none());
}

fn sandbox_list_filters<S: Store>(make: &impl Fn() -> S) {
    let s = make();
    let mut a = sb("a", "ua", SandboxState::Ready);
    a.labels.insert("env".into(), "prod".into());
    let mut b = sb("b", "ub", SandboxState::NotReady);
    b.labels.insert("env".into(), "dev".into());
    s.put_sandbox(&a).unwrap();
    s.put_sandbox(&b).unwrap();

    // 空 filter → 全部
    let all = s.list_sandboxes(&SandboxFilter::default()).unwrap();
    assert_eq!(all.len(), 2);

    // by id
    let f = SandboxFilter {
        id: Some("a".into()),
        ..Default::default()
    };
    let r = s.list_sandboxes(&f).unwrap();
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].sandbox_id, "a");

    // by label_selector
    let f = SandboxFilter {
        label_selector: [("env".to_string(), "prod".to_string())]
            .into_iter()
            .collect(),
        ..Default::default()
    };
    let r = s.list_sandboxes(&f).unwrap();
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].sandbox_id, "a");

    // by state
    let f = SandboxFilter {
        state: Some(SandboxState::NotReady),
        ..Default::default()
    };
    let r = s.list_sandboxes(&f).unwrap();
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].sandbox_id, "b");

    // by pod_uid（CRI 不下发，但 store 须支持）
    let f = SandboxFilter {
        pod_uid: Some("ub".into()),
        ..Default::default()
    };
    let r = s.list_sandboxes(&f).unwrap();
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].sandbox_id, "b");

    // 组合：label + state
    let f = SandboxFilter {
        label_selector: [("env".to_string(), "prod".to_string())]
            .into_iter()
            .collect(),
        state: Some(SandboxState::Ready),
        ..Default::default()
    };
    assert_eq!(s.list_sandboxes(&f).unwrap().len(), 1);
}

fn sandbox_delete_idempotent<S: Store>(make: &impl Fn() -> S) {
    let s = make();
    s.put_sandbox(&sb("sb1", "uid1", SandboxState::Ready))
        .unwrap();
    s.delete_sandbox("sb1").unwrap();
    match s.get_sandbox("sb1") {
        Err(StoreError::NotFound(_)) => {}
        other => panic!("expected NotFound after delete, got {other:?}"),
    }
    // 删不存在 = Ok（幂等）
    s.delete_sandbox("sb1").unwrap();
    s.delete_sandbox("never").unwrap();
}

// ---- container CRUD --------------------------------------------------------

fn container_crud<S: Store>(make: &impl Fn() -> S) {
    let s = make();
    let r = ct("c1", "sb1", ContainerState::Created);
    s.put_container(&r).unwrap();
    let got = s.get_container("c1").unwrap();
    assert_eq!(got.container_id, "c1");
    assert_eq!(got.sandbox_id, "sb1");

    let mut r2 = r.clone();
    r2.state = ContainerState::Running;
    s.put_container(&r2).unwrap();
    assert_eq!(
        s.get_container("c1").unwrap().state,
        ContainerState::Running
    );

    match s.get_container("nope") {
        Err(StoreError::NotFound(_)) => {}
        other => panic!("expected NotFound, got {other:?}"),
    }
}

fn container_list_filters<S: Store>(make: &impl Fn() -> S) {
    let s = make();
    let mut c1 = ct("c1", "sb1", ContainerState::Running);
    c1.labels.insert("tier".into(), "web".into());
    let mut c2 = ct("c2", "sb1", ContainerState::Exited);
    c2.labels.insert("tier".into(), "worker".into());
    let c3 = ct("c3", "sb2", ContainerState::Created);
    s.put_container(&c1).unwrap();
    s.put_container(&c2).unwrap();
    s.put_container(&c3).unwrap();

    let all = s.list_containers(&ContainerFilter::default()).unwrap();
    assert_eq!(all.len(), 3);

    // by sandbox_id
    let f = ContainerFilter {
        sandbox_id: Some("sb1".into()),
        ..Default::default()
    };
    assert_eq!(s.list_containers(&f).unwrap().len(), 2);

    // by id
    let f = ContainerFilter {
        id: Some("c3".into()),
        ..Default::default()
    };
    let r = s.list_containers(&f).unwrap();
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].container_id, "c3");

    // by label
    let f = ContainerFilter {
        label_selector: [("tier".to_string(), "web".to_string())]
            .into_iter()
            .collect(),
        ..Default::default()
    };
    assert_eq!(s.list_containers(&f).unwrap().len(), 1);

    // by state
    let f = ContainerFilter {
        state: Some(ContainerState::Exited),
        ..Default::default()
    };
    assert_eq!(s.list_containers(&f).unwrap().len(), 1);
}

fn container_delete_idempotent<S: Store>(make: &impl Fn() -> S) {
    let s = make();
    s.put_container(&ct("c1", "sb1", ContainerState::Created))
        .unwrap();
    s.delete_container("c1").unwrap();
    match s.get_container("c1") {
        Err(StoreError::NotFound(_)) => {}
        other => panic!("expected NotFound after delete, got {other:?}"),
    }
    s.delete_container("c1").unwrap();
    s.delete_container("never").unwrap();
}

// ---- IPAM ------------------------------------------------------------------

fn ipam<S: Store>(make: &impl Fn() -> S) {
    let s = make();
    // /30 → 2 个可分配（.1 .2），.0 网络 / .3 广播
    let l1 = s.lease_ip("10.244.0.0/30").unwrap();
    assert_eq!(l1.cidr, "10.244.0.0/30");
    let l2 = s.lease_ip("10.244.0.0/30").unwrap();
    assert_ne!(l1.ip, l2.ip);

    // 第三次 → 耗尽
    match s.lease_ip("10.244.0.0/30") {
        Err(StoreError::Db(_)) => {}
        other => panic!("expected ipam exhaustion (Db), got {other:?}"),
    }

    // 释放后可重用
    s.release_ip(&l1).unwrap();
    let l3 = s.lease_ip("10.244.0.0/30").unwrap();
    assert_eq!(l3.ip, l1.ip, "released ip should be reusable");

    // 释放不存在的租约 = Ok（幂等）
    s.release_ip(&IpLease {
        ip: "10.244.0.99".into(),
        cidr: "10.244.0.0/30".into(),
        sandbox_id: String::new(),
    })
    .unwrap();
}

// ---- transaction -----------------------------------------------------------

fn transaction_commit_and_rollback<S: Store>(make: &impl Fn() -> S) {
    let s = make();

    // 提交：两条写都可见
    s.transaction(Box::new(|t| {
        t.put_sandbox(&sb("sb1", "uid1", SandboxState::Ready))?;
        t.put_container(&ct("c1", "sb1", ContainerState::Created))?;
        Ok(())
    }))
    .unwrap();
    assert!(s.get_sandbox("sb1").is_ok());
    assert!(s.get_container("c1").is_ok());

    // 回滚：闭包返回 Err，两条写都不可见
    s.transaction(Box::new(|t| {
        t.put_sandbox(&sb("sb2", "uid2", SandboxState::Ready))?;
        t.put_container(&ct("c2", "sb2", ContainerState::Created))?;
        Err(StoreError::Other("boom".into()))
    }))
    .unwrap_err();
    match s.get_sandbox("sb2") {
        Err(StoreError::NotFound(_)) => {}
        other => panic!("expected rollback → NotFound, got {other:?}"),
    }
    match s.get_container("c2") {
        Err(StoreError::NotFound(_)) => {}
        other => panic!("expected rollback → NotFound, got {other:?}"),
    }

    // 分组：delete_sandbox + release_ip 原子
    let s2 = make();
    let lease = s2.lease_ip("10.244.0.0/24").unwrap();
    let mut sb_rec = sb("sb1", "uid1", SandboxState::Ready);
    sb_rec.pod_ip = lease.ip.clone();
    s2.put_sandbox(&sb_rec).unwrap();
    let lease_clone = lease.clone();
    s2.transaction(Box::new(move |t| {
        t.delete_sandbox("sb1")?;
        t.release_ip(&lease_clone)?;
        Ok(())
    }))
    .unwrap();
    assert!(s2.get_sandbox("sb1").is_err());
    // 释放的 ip 可重新分配
    let again = s2.lease_ip("10.244.0.0/24").unwrap();
    assert_eq!(again.ip, lease.ip);

    // 空事务 = Ok
    s.transaction(Box::new(|_t| Ok(()))).unwrap();
}

// ---- 红线 1：metadata/labels/annotations 原样往返 --------------------------

fn metadata_passthrough<S: Store>(make: &impl Fn() -> S) {
    let s = make();
    let mut r = sb("sb1", "uid1", SandboxState::Ready);
    r.metadata = SandboxMetadata {
        name: "pod-name".into(),
        namespace: "kube-system".into(),
        uid: "uid-xyz".into(),
        attempt: 7,
    };
    r.labels.insert("a/b".into(), "v1".into());
    r.labels.insert("k".into(), "v2".into());
    r.annotations
        .insert("agent-sandbox/type".into(), "1".into());
    s.put_sandbox(&r).unwrap();
    let got = s.get_sandbox("sb1").unwrap();
    assert_eq!(got.metadata, r.metadata);
    assert_eq!(got.labels, r.labels);
    assert_eq!(got.annotations, r.annotations);

    let mut c = ct("c1", "sb1", ContainerState::Exited);
    c.exit_code = 137;
    c.reason = Some(oas_types::ContainerExitReason::OomKilled);
    c.message = "oom".into();
    c.labels.insert("app".into(), "x".into());
    s.put_container(&c).unwrap();
    let cg = s.get_container("c1").unwrap();
    assert_eq!(cg.exit_code, 137);
    assert_eq!(cg.reason, Some(oas_types::ContainerExitReason::OomKilled));
    assert_eq!(cg.message, "oom");
    assert_eq!(cg.labels, c.labels);
}
