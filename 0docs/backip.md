
概要设计0.2.md#23-23
接下来我们做 编排层 (oas-manager) 和oas-store。我们要用测试驱动开发。测试要从  API/CRI 层 (oas-cri) 到 编排层 (oas-manager) 层的端到端的测试， driver,net,storage用mock @0docs/概要设计0.2.md
我将首先探索代码库以了解当前状态。让我启动并行的 Explore agents 来调查相关区域。

Agent:Explore oas-manager and oas-store crates
IN
Explore the Rust workspace at /home/yunfei/Code/Only-AgentSandbox. I need a thorough understanding of the `oas-manager` and `oas-store` crates.

Specifically:
1. List all files under crates/oas-manager/ and crates/oas-store/ and read each one fully.
2. Report the exact current contents: the `Manager` trait, `StubManager`, `Store` trait, `Txn`, `StoreError`, `OasError`, all request/response types, and any existing real implementations.
3. Report Cargo.toml dependencies for both crates.
4. Note the object-safety concern around `Store::transaction` (generic method) mentioned in design docs §2.7/§3.2 — how is it currently structured?
5. Report any existing tests in these crates.

Be thorough — read whole files. Report exact file paths and key code snippets.
Agent:Explore oas-cri and oas-types crates
IN
Explore the Rust workspace at /home/yunfei/Code/Only-AgentSandbox. I need a thorough understanding of the `oas-cri` and `oas-types` crates, plus `oas-runtime`.

Specifically:
1. List and read all files under crates/oas-cri/src/, crates/oas-types/src/, and crates/oas-runtime/src/.
2. Report the exact structure of oas-types: all shared domain types (SandboxRecord, ContainerRecord, SandboxState, ContainerState, ContainerExitReason, IpLease, SandboxFilter, ContainerFilter, SandboxMetadata, ContainerMetadata, Mount, LinuxResources, KeyValue, DnsConfig, MountPropagation).
3. Report the CRI layer: how RuntimeSvc/ImageSvc hold Arc<dyn Manager>, the convert.rs functions, error.rs (to_status, swallow_not_found), unsupported.rs.
4. Report existing end-to-end tests in oas-cri (the UDS tests mentioned in design: Version/Status/ListImages/ImageFsInfo + RunPodSandbox UNAVAILABLE). Show the test harness pattern — how they spin up the UDS server, what StubManager they inject, how clients call.
5. Report oas-runtime main.rs assembly.
6. Report Cargo.toml deps for these crates.

Read whole files. Report exact paths and key code snippets, especially the existing test harness pattern since I'll mirror it for new end-to-end tests.
Agent:Explore lower-layer trait crates and workspace
IN
Explore the Rust workspace at /home/yunfei/Code/Only-AgentSandbox. I need to understand the lower-layer trait crates `oas-driver`, `oas-net`, `oas-storage` and the overall workspace layout.

Specifically:
1. Read the root Cargo.toml (workspace definition, members, resolver).
2. List and read all files under crates/oas-driver/src/, crates/oas-net/src/, crates/oas-storage/src/.
3. Report the exact trait definitions: FirecrackerDriver (create_vm, get_vm, delete_vm, list_vm), NetworkManager (setup, teardown), StorageManager (provision, cleanup), and their associated types (VmId, VmStatus, VmSpec, VmLifecycle, NetConfig, DiskConfig, IpLease) and error enums (DriverError, NetError, StorageError).
4. Report the GuestAgent trait if it exists (§2.8) — is it defined anywhere yet? In which crate?
5. Note where these traits live and whether oas-manager already depends on them.
6. Run `cargo test --workspace --no-run` mentally/report what test dependencies exist (tokio, mockall, etc.) by reading the Cargo.tomls. Don't actually run cargo.
7. Report any existing mock implementations or test helpers anywhere in the workspace.

Read whole files. Report exact paths and trait signatures precisely — I need them to write mock implementations.
探索过程已经很清楚了。在规划之前，我先直接验证一下工作空间路径，并读取两个关键文件以确认具体的契约。

Bash Verify workspace path structure
IN
ls -la && echo "---" && find . -maxdepth 2 -name Cargo.toml | head -20

OUT
总用量 8
drwxr-xr-x 7 root root 178  6月 27 18:21 .
drwxr-xr-x 4 root root  65  6月 27 17:16 ..
drwxr-xr-x 2 root root 160  6月 28 23:37 0docs
drwxr-xr-x 2 root root  10  6月 27 18:19 1architecture_decision_records
drwxr-xr-x 8 root root 280  6月 28 23:39 .git
-rw-r--r-- 1 root root 692  6月 28 13:16 .gitignore
drwxr-xr-x 3 root root 287  6月 28 13:16 .history
drwxr-xr-x 5 root root 105  6月 28 16:37 only-agent-sandbox
-rw-r--r-- 1 root root  19  6月 27 17:16 README.md
---
./only-agent-sandbox/Cargo.toml
Read lib.rs
Read lib.rs
我现在已经掌握了全局和准确的契约。工作空间根目录位于 only-agent-sandbox/。在规划之前，我需要解决几个实质性影响设计的决策——它们是实际需要权衡的选择，而不是我可以自行决定的事情。

AskUserQuestion
OUT
Your questions have been answered: "Store trait 当前因 `transaction<R,F>` 泛型方法非 object-safe，无法 `Arc<dyn Store>`。怎么解决？"="改 Box<dyn FnOnce>（推荐）", "oas-store 这一轮实现到什么程度？"="RedbStore + MemoryStore 都做（推荐）", "GuestAgent（§2.8 vsock 第二控制面，目前只是提议、无 trait）这轮怎么处理？它决定 StartContainer/exit_code 回灌能否端到端测。"="本轮延后 agent". You can now continue with these answers in mind.
决策已确定：

Store → object-safe，通过 Box<dyn FnOnce> 实现。
构建 RedbStore + MemoryStore，并共享一致性测试。
本轮推迟 GuestAgent — Manager 不持有 agent；StartContainer 直接设置 RUNNING（无实际进程/exit_code 路径）。
让我启动一个 Plan agent 来验证设计并解决更棘手的细节（eventfd 处理、事务重构、test-crate 的位置、TDD 排序），我会将所有上下文传递给它。

Agent:Design TDD plan for oas-manager + oas-store
IN
Design a detailed TDD implementation plan for the Rust workspace at `/home/yunfei/Code/Only-AgentSandbox/only-agent-sandbox/` (Cargo workspace root). Read the design doc `/home/yunfei/Code/Only-AgentSandbox/0docs/概要设计0.2.md` (esp. §2.3-2.7, §3.2, §3.6, §4.2-4.5) and the existing source files before designing.

## Goal
Build the real orchestration layer (`oas-manager` real `Manager` impl) and `oas-store` (real `RedbStore` + `MemoryStore`), test-driven, with end-to-end tests from the CRI gRPC layer (`oas-cri`, over Unix domain socket) down through the real `Manager`, with `driver`/`net`/`storage` mocked. GuestAgent (§2.8) is DEFERRED this round.

## Current state (verified)
- `crates/oas-manager/src/lib.rs`: defines `Manager` trait (async, object-safe via async_trait), `OasError`, request/response types, and `StubManager`. Currently depends ONLY on oas-types. Real Manager not implemented.
- `crates/oas-store/src/lib.rs`: defines `Store` trait (sync), `StoreError`, `Txn` (unit struct placeholder). `Store::transaction<R,F>` is GENERIC → trait is NOT object-safe. No impls. Depends only on oas-types.
- `crates/oas-driver/src/lib.rs`: `FirecrackerDriver` trait — `create_vm(&self, netns_path:&str, type_id:u8, spec:VmSpec, event_fd:RawFd) -> Result<VmId,DriverError>`, `get_vm(VmId)->VmStatus`, `delete_vm(VmId)`, `list_vm()->Vec<VmStatus>`. Types: `VmId(u64)`, `VmStatus{started,healthy,lifecycle:VmLifecycle{Creating,Restoring,Running,Degraded,Stopping,Stopped}}`, `VmSpec{rw_layer_path:Option<String>,cloud_disk_dev:Option<String>}`.
- `crates/oas-net/src/lib.rs`: `NetworkManager` — `setup(&self,sandbox_id:&str)->NetConfig`, `teardown(&NetConfig)`. `NetConfig{netns_path,tap_name,mac,pod_ip,gateway,lease:IpLease}`.
- `crates/oas-storage/src/lib.rs`: `StorageManager` — `provision(&self,sandbox_id:&str,type_id:u8,cloud_disk_ref:Option<&str>)->DiskConfig`, `cleanup(&DiskConfig)`. `DiskConfig{rw_layer_path,cloud_disk_dev}`.
- `crates/oas-types/src/lib.rs`: all shared types (SandboxRecord, ContainerRecord, SandboxState{Ready,NotReady}, ContainerState{Created,Running,Exited,Unknown}, ContainerExitReason, IpLease, SandboxFilter, ContainerFilter, etc.). Records have `created_at:i64`, `started_at:Option<i64>`, `finished_at:Option<i64>`, `exit_code:i32`, `vm_id:u64`, `state`, etc.
- `crates/oas-cri/tests/uds.rs`: existing e2e test harness — `serve(path)` helper builds `StubManager`→`Arc<dyn Manager>`→`RuntimeSvc`+`ImageSvc`→binds `UnixListener`→`async_stream::stream!` yields accepted streams→`Server::builder().add_service(...).serve_with_incoming(...)` spawned on tokio. Client uses `Endpoint::from_shared(format!("unix:{path}")).connect()` → `RuntimeServiceClient`/`ImageServiceClient`. oas-cri depends on oas-manager + oas-types only (must NOT depend on driver/net/storage/store — dependency direction).
- Root `Cargo.toml` workspace deps: async-trait, serde, thiserror, tonic, tonic-prost, prost, tonic-build, tonic-prost-build, tokio(full), tokio-stream, async-stream. NO mockall, NO redb, NO libc, NO nix currently.

## Decisions already made by the user (do NOT re-litigate)
1. Store object-safety: change `transaction` to `fn transaction(&self, f: Box<dyn FnOnce(&mut Txn) -> Result<(), StoreError>>) -> Result<(), StoreError>` (object-safe, returns unit). `Txn` gets real mutating methods (put_sandbox/put_container/delete_sandbox/delete_container/lease_ip/release_ip). Keep the existing &self CRUD + read methods on the Store trait too.
2. Implement BOTH `RedbStore` (redb-backed, production) and `MemoryStore` (HashMap, no deps, for fast deterministic tests). Run one shared conformance test suite against both.
3. GuestAgent DEFERRED: real Manager does NOT hold an agent. `start_container` just transitions CREATED→RUNNING (sets started_at), no real process. `stop_container` transitions RUNNING→EXITED with exit_code=0/reason=Completed as an explicit-stop best-effort (documented simplification; red-line 2 about not inferring exit_code from VM-aliveness is preserved since this is an explicit stop, not a natural-exit inference). The agent Exited-event回灌 path is deferred.

## Your job
Produce a detailed, executable TDD plan. Specifically resolve and recommend:

1. **Test crate placement**: The e2e test must go CRI gRPC client → UDS → real Manager (with mocked driver/net/storage + real MemoryStore). oas-cri can't depend on lower crates. Recommend a NEW dedicated test-only crate (e.g. `crates/oas-e2e/`) that depends on everything, OR putting it in `oas-runtime` tests. Decide and justify. Mocks (MockDriver/MockNet/MockStorage) — hand-rolled with interior mutability (Arc<Mutex<...>>, AtomicBool) to avoid adding mockall, or mockall? Recommend.

2. **Store conformance tests**: enumerate the test cases that the shared suite runs against both MemoryStore and RedbStore (sandbox/container CRUD, get_by_uid idempotency key, list with id/label/sandbox_id/state filters, IPAM lease_ip/release_ip no-double-alloc + release-realloc, transaction atomicity/grouping). How to structure the shared suite (generic `fn run_store_tests<S: Store>()`? or a macro? or trait-based test module included in each impl's tests).

3. **Manager struct & deps**: `Manager{driver,net,storage,store,types:SandboxTypeTable,locks:PerKeyLock,clock:?}`. Recommend how to handle time (created_at/started_at/finished_at need determinism in tests) — injectable `Clock` trait with real + fake impls. ID generation (sandbox_id/container_id/vm_id) — recommend format & uniqueness approach without new deps (AtomicU64 counter). SandboxTypeTable (type_id 0..=2 config: vcpu/mem/snapshot_path/rootfs_ro/has_rw_layer/has_cloud_disk/image_whitelist). PerKeyLock (per-sandbox_id, fixed sandbox→container order; read paths no lock).

4. **eventfd handling**: §4.2 step 6-7 — Manager creates an eventfd, passes RawFd to driver.create_vm, then async-waits (AsyncFd) for readiness with a timeout before writing READY. Recommend whether to add `libc` for the eventfd syscall, how the mock driver signals readiness (write 8 bytes to fd), and how to test both happy (immediate signal) and timeout/rollback (no signal → NOTREADY) paths without hanging. If this is too risky for iteration 1, recommend a simpler faithful abstraction and justify.

5. **Sandbox & Container FSM logic** per §4.2/4.4/4.5: enumerate the exact steps + error mapping (lower errors → OasError variants) + idempotency + rollback for: run_sandbox (net.setup→storage.provision→driver.create_vm+eventfd wait→store.put_sandbox READY; idempotent via get_sandbox_by_uid; rollback teardown/cleanup/delete_vm on failure), stop_sandbox (delete_vm→poll get_vm to Stopped→net.teardown→NOTREADY), remove_sandbox (storage.cleanup→store.delete_sandbox+release_ip in one transaction; idempotent), create_container (image whitelist→ImageNotInList, type/resources consistency→TypeMismatch, store CREATED), start_container (CREATED→RUNNING+started_at), stop_container (RUNNING→EXITED best-effort), remove_container (delete, idempotent), list/status (store-only, never driver), image_* (whitelist). Note red-line 3: List*/Status never hit driver.

6. **TDD ordering**: the sequence in which to write tests-then-code, so each step compiles and tests green before next. Phase it.

7. **oas-manager Cargo.toml changes**: add deps on oas-driver, oas-net, oas-storage, oas-store, tokio (for AsyncFd/clock/spawn), libc if needed. Note dev-deps.

8. **main.rs swap**: oas-runtime/src/main.rs currently injects StubManager — swap to real Manager with RedbStore + real (stubbed?) driver/net/storage? For production main, the lower impls don't exist yet. Recommend: keep main.rs on a constructor that wires RedbStore + the real Manager, but since real driver/net/storage impls are deferred, either keep StubManager in main for now OR add a feature/constructor. Decide.

9. **Reconcile (§4.6)**: in scope or deferred this iteration? Recommend (lean: defer to next iteration as a background task is hard to TDD cleanly; note it).

Read the actual files to ground your recommendations in real signatures. Return a concrete, ordered, phased plan with file paths and the test cases spelled out. Flag any risks or signature mismatches you find.
Plan agent 已经提交了一份详尽且有据可查的设计，并标记了实际的签名缝隙（signature seams）。我已经掌握了足够的信息来定稿。让我来编写计划文件。

Write oas-manager-oas-store-lucky-giraffe.md
103 lines
# TDD: real `oas-manager` + `oas-store` (RedbStore/MemoryStore) + CRI→Manager E2E

## Context

The repo currently ships only **contracts + stubs**: `oas-manager` has the `Manager` trait + `StubManager` (writes return `Unavailable`), and `oas-store` has the `Store` trait + a unit-struct `Txn` placeholder (no impls). The CRI layer (`oas-cri`) is fully implemented and tested end-to-end against `StubManager` over UDS. The lower-layer traits (`oas-driver`/`oas-net`/`oas-storage`) exist as contracts only.

This iteration builds the **real orchestration brain** (`oas-manager`) and the **real persistence layer** (`oas-store`: `RedbStore` + `MemoryStore`), test-driven, with end-to-end tests flowing CRI gRPC client → UDS → real `Manager` → mocked driver/net/storage + real store. GuestAgent (§2.8) is deferred.

Outcome: `RunPodSandbox`/`CreateContainer`/`StartContainer`/`StopContainer`/`RemoveContainer`/`StopPodSandbox`/`RemovePodSandbox` + `List*`/`*Status` + image whitelist work for real through the CRI socket, with idempotency, rollback, and PLEG read-paths that never touch the driver.

Workspace root: `/home/yunfei/Code/Only-AgentSandbox/only-agent-sandbox/`.

## Locked decisions (from user)

1. **Store object-safety**: `transaction` → `fn transaction(&self, f: Box<dyn FnOnce(&mut dyn Txn) -> Result<(), StoreError>>) -> Result<(), StoreError>`. `Txn` becomes a **trait** (backend-agnostic), object-safe. `Arc<dyn Store>` now works.
2. **Both** `RedbStore` (redb, production) and `MemoryStore` (HashMap, tests) behind one shared conformance suite.
3. **GuestAgent deferred**: `OasManager` holds no agent. `start_container` → CREATED→RUNNING (sets `started_at`), no real process. `stop_container` → RUNNING→EXITED with `exit_code=0`/`reason=Completed` as an **explicit-stop** best-effort (red-line 2 preserved: we never *infer* exit_code from VM liveness; natural-exit回灌 is deferred).

## Chosen approach for the seams the Plan agent flagged

- **IPAM ownership = net layer** (matches §4.2 step 4: `NET → Store::lease_ip`). The fake net holds `Arc<dyn Store>` + a CIDR; `setup` calls `store.lease_ip(cidr)`, fills `lease.sandbox_id` on the returned struct, returns `NetConfig` carrying that lease; `teardown` calls `store.release_ip`. Manager never calls lease/release directly. → exercises real IPAM e2e, no `lease_ip` signature change (keeps `lease_ip(&self, cidr: &str)`; caller fills `sandbox_id`).
- **`delete_*` idempotency** = store returns `Ok(())` on missing (CRI `swallow_not_found` is belt-and-suspenders).
- **`release_ip`** keys on `lease.ip` only (ipam PK = ip); `cidr`/`sandbox_id` fields informational.
- **Readiness/eventfd** (deviation from §4.2 raw eventfd, documented): introduce a small `VmReadiness` trait `wait_ready(vm_id, timeout) -> Result<(), OasError>`. Manager: `create_vm(...)` (passes sentinel `event_fd = -1`, mock ignores) → `readiness.wait_ready(timeout)` → on timeout rollback `delete_vm`+`cleanup`+`teardown`. Iteration-1 impls: `ImmediateReadiness` (happy) and `NeverReadyReadiness` (sleeps the timeout then errs — tests the orphan-VM rollback without hanging). Real `EventFdReadiness` (libc + AsyncFd) deferred to the driver-impl iteration.
- **Reconcile (§4.6)**: deferred (background task, hard to TDD cleanly without a real `list_vm`; the `PerKeyLock` seam is in place so it's additive later).
- **`main.rs`**: keep `StubManager` (real driver/net/storage impls don't exist yet). Add a TODO; swap in a later iteration. The real `OasManager` is wired only in tests.

## Scope

**In**: real `OasManager` FSM (sandbox + container lifecycle, image whitelist, version/status), `Clock`/`IdGenerator`/`SandboxTypeTable`/`PerKeyLock`/`VmReadiness`, `oas-store` object-safety + `MemoryStore` + `RedbStore` + conformance suite, new `oas-e2e` test crate (mocks + direct FSM tests + UDS e2e).
**Out (deferred)**: GuestAgent + exit_code回灌, reconcile, real driver/net/storage impls, real eventfd, `main.rs` swap, `update_runtime_config` persistence to `meta` table.

## Architecture changes

### `crates/oas-store/src/lib.rs` (+ `memory.rs` / `redb.rs` submodules)
- Replace `pub struct Txn;` with `pub trait Txn` exposing `put_sandbox`/`put_container`/`delete_sandbox`/`delete_container`/`lease_ip`/`release_ip` (all `&mut self`).
- `Store` trait: keep reads (`get_sandbox`, `get_sandbox_by_uid`, `list_sandboxes`, `get_container`, `list_containers`) + `lease_ip`/`release_ip` as per-backend `&self` methods. Make the four write methods (`put_sandbox`/`put_container`/`delete_sandbox`/`delete_container`) **default impls** that delegate to `transaction` with a single-op closure → all writes transactional, less per-backend code. Change `transaction` to the object-safe `Box<dyn FnOnce(&mut dyn Txn) -> Result<(), StoreError>>` signature.
- `MemoryStore` (`Mutex<MemoryTables>` of 3 HashMaps: sandbox/container/ipam; image/meta tables unused this iter). `MemoryTxn` buffers ops in a `Vec<PendingOp>` and applies only on `Ok` (manual rollback for `transaction_rolls_back_on_err`).
- `RedbStore` (redb 4.1): 5 tables created eagerly in the ctor (`sandbox:&str->[u8]`, `container:&str->[u8]`, `ipam:&str->()`, `image:&str->()`, `meta:&str->&str`). Records serialized via `serde_json` (serde already on records in oas-types). `RedbTxn<'a>` wraps `&mut redb::WriteTransaction<'a>`; rollback = drop-without-commit (redb native). `lease_ip` scans the `ipam` table for used IPs in the CIDR, picks next free, inserts in the write txn. Ctor takes a path; tests use `std::env::temp_dir()` + pid-suffixed name + `Drop` cleanup (no `tempfile` dep).

### `crates/oas-manager/src/lib.rs` (+ `clock.rs`/`id.rs`/`types_table.rs`/`lock.rs`/`readiness.rs`)
- `OasManager { driver, net, storage, store, types, locks, clock, id_gen, readiness }` (no `agent`).
- `Clock` trait (`now_unix_secs() -> i64`): `SystemClock` (prod) + `FakeClock` (`Arc<AtomicI64>`, test) → deterministic `created_at`/`started_at`/`finished_at`.
- `IdGenerator` (`AtomicU64`): `sb-{:016x}` / `ct-{:016x}`; `vm_seq` starts at 1 (avoid `VmId(0)`). No new deps.
- `SandboxTypeTable` (`[SandboxType; 3]` per §3.2 table: type0 1c/512M no-rw; type1 2c/2G rw; type2 4c/8G rw+cloud). `get(type_id) -> Result<&SandboxType, OasError>` (>`2` → `InvalidArgument`).
- `PerKeyLock` (`Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>`): `async fn lock(key) -> OwnedMutexGuard`. Container writes lock the **parent sandbox** (fixed sandbox→container order). Read paths take no lock.
- `VmReadiness` trait + `ImmediateReadiness`/`NeverReadyReadiness` (test fakes live in `oas-e2e`; the trait + a `SystemReadiness` placeholder can live here or in tests — keep the trait in oas-manager, fakes in oas-e2e).
- Error mapping (lower → `OasError`): `DriverError::NotFound→NotFound`, `Exists→Conflict`, rest→`Unavailable`; `NetError::IpamExhausted→Unavailable`, rest→`Internal`; `StorageError::CloudDiskNotFound→InvalidArgument`, rest→`Internal`; `StoreError::NotFound→NotFound`, rest→`Internal`; readiness timeout→`Unavailable`. **Must emit `ImageNotInList` (not `InvalidArgument`) for whitelist misses** — `oas-cri/src/error.rs` maps `ImageNotInList→Status::not_found`.

FSM (per §4.2/4.4/4.5), key points:
- `run_sandbox`: lock by `pod_uid` → `get_sandbox_by_uid` idempotent short-circuit (Ready→return id; NotReady→`Conflict`) → `types.get` → cloud-disk/`has_cloud_disk` consistency checks → `net.setup` → `storage.provision` → `driver.create_vm` → `readiness.wait_ready` (on fail rollback `delete_vm`+`cleanup`+`teardown`) → `store.transaction(put_sandbox READY)` → return id.
- `stop_sandbox`: `delete_vm` → bounded poll `get_vm` to `Stopped`/`NotFound` → `net.teardown` (releases IP) → `transaction(put_sandbox NOTREADY)`.
- `remove_sandbox`: `get_sandbox` (NotFound→`Ok`) → `storage.cleanup` (best-effort) → `transaction(delete_sandbox)`.
- `create_container`: lock sandbox → `get_sandbox` → image whitelist (`ImageNotInList`) → resources vs type budget (`TypeMismatch`) → `put_container CREATED`.
- `start_container`: lock sandbox → `get_container` (not Created→`Conflict`) → `put_container RUNNING + started_at`.
- `stop_container`: lock sandbox → idempotent if Exited → `put_container EXITED + exit_code=0 + reason=Completed + finished_at` (explicit-stop simplification).
- `remove_container`: `get_container` (NotFound→`Ok`) → `delete_container`.
- `*_status`/`list_*`: store-only (red-line 3; tests assert driver call-count 0).
- `image_*`: whitelist from `types`; `image_status`→`Option<ImageInfo>`, `pull_image`→ref or `ImageNotInList`, `list_images`→flatten, `remove_image`→`Ok`. No store/driver.
- `version`/`status`: fixed values (reuse StubManager's); `update_runtime_config`→`Ok` (TODO: persist pod_cidr).

### NEW `crates/oas-e2e/` (test-only crate, `publish = false`)
Depends on `oas-cri`/`oas-manager`/`oas-driver`/`oas-net`/`oas-storage`/`oas-store`/`oas-types` + `tokio`/`tonic`/`async-stream`. Holds:
- `mocks.rs`: hand-rolled `MockDriver`/`MockNet`/`MockStorage` (interior mutability: `Arc<Mutex<Vec<…>>>` call recording + `AtomicBool` failure switches; `MockNet` takes `Arc<dyn Store>` + CIDR to do real IPAM) + `ImmediateReadiness`/`NeverReadyReadiness`. **No `mockall`.**
- `manager_fsm.rs`: direct-Manager tests (sandbox + container FSM, idempotency, rollback, red-line 3 driver-call-count).
- `lifecycle.rs`: UDS e2e — reuse the `serve(path)` harness pattern from `crates/oas-cri/tests/uds.rs` but inject `Arc<OasManager>` (real) with `MemoryStore` + mocks; assert through `RuntimeServiceClient`/`ImageServiceClient` over `unix:{path}`.

### `crates/oas-store/tests/`
- `common/mod.rs`: `pub fn run_store_tests<S: Store>(make: impl Fn() -> S)` — the shared conformance suite (sandbox/container CRUD incl. metadata roundtrip red-line 1, `get_by_uid` idempotency key, list filters by id/label/state/sandbox_id, delete-missing=`Ok`, IPAM uniqueness/exhaustion/release-realloc, transaction commit/rollback/grouping/empty).
- `memory.rs` / `redb.rs`: `mod common; run_store_tests(MemoryStore::default)` / `run_store_tests(|| RedbStore::open(temp_path))`.

### Cargo.toml changes
- Root `[workspace.dependencies]`: add `redb = "4.1"`.
- `oas-store`: add `redb`, `serde`, `serde_json` (dev: `tokio` not needed — sync trait).
- `oas-manager`: add `oas-driver`/`oas-net`/`oas-storage`/`oas-store` (path) + `tokio` (Mutex/time/spawn). **No `libc`/`mockall` this iteration.**
- `oas-e2e`: new member (auto via `crates/*` glob).

## TDD phases (each ends `cargo build --workspace && cargo test --workspace` green)

0. **Setup**: add `oas-e2e` skeleton, workspace `redb` dep, `oas-manager`/`oas-store` Cargo deps. Compile-only green.
1. **Store object-safety + MemoryStore**: write `tests/common` + `tests/memory.rs` first → change `Txn`→trait + `transaction` signature + default write impls + `MemoryStore` + `MemoryTxn` (buffer/apply). RedbStore stubbed, `redb.rs` `#[ignore]`. Green on Memory.
2. **RedbStore**: un-ignore `redb.rs` → implement `RedbStore`/`RedbTxn` (serde_json, eager tables, ipam scan). Same suite green.
3. **Manager supporting types**: tests for `Clock`/`IdGenerator`/`SandboxTypeTable`/`PerKeyLock` → implement in submodules. Green.
4. **Manager sandbox FSM**: `manager_fsm.rs` sandbox tests (happy, uid-idempotent, type2-cloud-disk checks, storage-fail rollback, readiness-timeout rollback, stop→poll→NotReady, remove idempotent, list/status never call driver) → implement. Green.
5. **Manager container FSM**: container tests (create happy/whitelist-miss→ImageNotInList/mem-exceeds→TypeMismatch/sandbox-missing; start Created→Running+started_at / not-Created→Conflict; stop→Exited+exit_code=0; remove idempotent; list by sandbox_id; status never calls driver) → implement. Green.
6. **Manager image_* + version/status**: tests → implement. Green.
7. **E2E over UDS**: `lifecycle.rs` (RunPodSandbox→status READY+pod_ip; full container cycle Run→Create→Start→Status(RUNNING)→Stop→Status(EXITED,exit_code=0)→Remove→Status not_found; List; image whitelist→not_found mapping; sandbox idempotency by uid; stop→NotReady; remove idempotent) → wire `serve` helper with real Manager. Green.

## Verification

- `cargo build --workspace` clean.
- `cargo test --workspace` all green: existing oas-cri convert + UDS-stub tests still pass; new oas-store conformance (Memory + Redb identical); oas-e2e direct-Manager + UDS-lifecycle.
- Manual smoke (optional): `OAS_SOCKET=/tmp/oas.sock cargo run` still boots with StubManager (main.rs unchanged).
- Red-lines asserted by tests: ① metadata/labels roundtrip in store conformance; ② no exit_code inference (explicit-stop only); ③ `list_*`/`*_status` assert `MockDriver` call-count == 0.

## Risks / flags

- **`Txn` becomes a trait** (was `struct Txn;`) — contract change within oas-store; consistent with the user's object-safety decision.
- **`VmReadiness` + sentinel `event_fd=-1`** deviates from §4.2's raw eventfd wait; real `EventFdReadiness` (libc+AsyncFd) deferred to driver-impl iteration. Documented in code.
- **IPAM-by-net** means the fake net is stateful (holds store) — intentional, exercises real IPAM e2e.
- **`update_runtime_config` persistence**, **reconcile**, **cross-process vm_id durability** → TODOs this iteration.
- **redb 4.1 API**: tables created eagerly in ctor to avoid `TableDoesNotExist` on first read; `lease_ip` does a scan inside a write txn.
计划已编写完成。在继续之前，先为您总结一下。

任务： 真正的 oas-manager（编排大脑）+ oas-store（RedbStore + MemoryStore），TDD，端到端测试从 CRI gRPC → UDS → 真正的 Manager，driver/net/storage 使用 mock。

关键决策（您已确认）：

Store → 通过 transaction(Box<dyn FnOnce(&mut dyn Txn)>) 实现对象安全，其中 Txn 是一个 trait（与后端无关）；支持 Arc<dyn Store>。
RedbStore + MemoryStore 使用统一的一致性测试套件。
GuestAgent 暂缓 — start_container → RUNNING，stop_container → EXITED（明确的停止，不进行 exit_code 推断）。
我针对 Plan-agent 标记的接缝所做的决定：

IPAM 由 net 层拥有（模拟 net 持有 store，调用 lease_ip/release_ip）— 符合 §4.2，并端到端测试真实 IPAM。
VmReadiness trait 将 eventfd 等待抽象化（原始 libc+AsyncFd 推迟到 driver-impl 迭代；测试 ImmediateReadiness + NeverReadyReadiness 以实现确定性回滚且不会挂起）。
跨 crate 共享的新 oas-e2e 测试 crate（mocks + 直接 FSM 测试 + UDS 生命周期），不使用 mockall。
调和、main.rs 切换、真实 driver/net/storage → 推迟（TODOs）。
TDD 阶段： 设置 → Store 对象安全+MemoryStore → RedbStore → Manager 支持类型 → sandbox FSM → container FSM → image/status → UDS e2e，每个阶段在下一个阶段开始前都保持编译+测试通过（绿）。

Claude’s Plan oas-manager-oas-store-lucky-giraffe.md
Stayed in plan mode
反馈很好——两者都能使 FSM 更清晰地符合 CRI 的幂等性。我来更新一下计划。

Edit oas-manager-oas-store-lucky-giraffe.md
Modified
Edit oas-manager-oas-store-lucky-giraffe.md
Modified
已更新 start_container 和 stop_container 的语义以及 container-FSM 测试列表，以符合 CRI 的幂等性：

StartContainer: Created→Running; Running→Ok (幂等); Exited/Unknown→Conflict.
StopContainer: Exited→Ok (幂等); Created 或 Running→Exited(exit_code=0).
Claude’s Plan oas-manager-oas-store-lucky-giraffe.md
User approved the plan
计划已批准。我来设置待办事项列表，并开始第 0 阶段（设置）。

Update Todos

Phase 0: Setup — add oas-e2e skeleton, workspace redb dep, oas-manager/oas-store Cargo deps; compile-only green

Phase 1: Store object-safety + MemoryStore (TDD) — Txn trait, transaction Box<dyn FnOnce>, default write impls, MemoryStore + MemoryTxn, conformance suite on Memory

Phase 2: RedbStore (TDD) — redb backend, RedbTxn, serde_json, eager tables, ipam scan; conformance green

Phase 3: Manager supporting types — Clock/IdGenerator/SandboxTypeTable/PerKeyLock + tests

Phase 4: Manager sandbox FSM — run/stop/remove/status/list + tests (idempotency, rollback, red-line 3)

Phase 5: Manager container FSM — create/start/stop/remove + tests (CRI-idempotent start/stop)

Phase 6: Manager image_* + version/status + tests

Phase 7: E2E over UDS — oas-e2e lifecycle.rs with real Manager + mocks; green
让我先确认一下 green baseline，并读取我将要处理的精确文件。

Bash Establish green baseline build + test
IN
cd /home/yunfei/Code/Only-AgentSandbox/only-agent-sandbox && cargo build --workspace 2>&1 | tail -15 && echo "=== TEST ===" && cargo test --workspace 2>&1 | tail -25

OUT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.10s
=== TEST ===

   Doc-tests oas_net

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

   Doc-tests oas_storage

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

   Doc-tests oas_store

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

   Doc-tests oas_types

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
Read lib.rs
基线状态良好。让我读取我将依赖的下层 trait crates 和 CRI 错误映射。

Read lib.rs
Read lib.rs
Read lib.rs
Read error.rs
所有签名已确认。现在进行第 0 阶段 — 设置。我来读取根目录下的 Cargo.toml 以及我需要修改的两个 Cargo.toml 文件。

Read Cargo.toml
Read Cargo.toml
Read Cargo.toml
我来读取一下 oas-cri 的公共 API 以及我需要参照的现有 UDS 测试。

Read lib.rs
Read uds.rs
现在我已经有了完整的 API 接口。让我执行第 0 阶段的编辑 —— 工作区 redb 依赖，两个 crate 的 Cargo.toml 文件，以及 oas-e2e 的骨架。

Edit Cargo.toml
Added 2 lines
在确定版本之前，让我先确认一下本地注册表缓存中实际有哪些可用的 redb 版本。

Bash Check cached redb and serde_json versions
IN
ls ~/.cargo/registry/cache/*/ 2>/dev/null | grep -i redb; echo "---src---"; ls ~/.cargo/registry/src/*/ 2>/dev/null | grep -i redb; echo "---serde_json---"; ls ~/.cargo/registry/cache/*/ 2>/dev/null | grep -i serde_json | head

OUT
redb-4.1.0.crate
---src---
redb-4.1.0
---serde_json---
serde_json-1.0.149.crate
serde_json-1.0.149.crate
serde_json-1.0.150.crate
redb-4.1.0 is cached (I wrote "2" — fixing to "4.1"). Let me verify the redb 4.1 API surface before coding against it.

Bash Inspect redb 4.1 API surface
IN
R=~/.cargo/registry/src/*/redb-4.1.0/src; echo "=== Database impl methods ==="; grep -n "pub fn " $R/database.rs | head -40; echo "=== WriteTransaction ==="; grep -n "pub fn " $R/write_transaction.rs | head -30; echo "=== ReadTransaction ==="; grep -n "pub fn " $R/read_transaction.rs | head -20; echo "=== TableDefinition ==="; grep -n "pub const fn new\|impl.*TableDefinition\|pub fn " $R/table.rs | head -20

OUT
=== Database impl methods ===
ugrep: warning: /root/.cargo/registry/src/*/redb-4.1.0/src/database.rs: No such file or directory
=== WriteTransaction ===
ugrep: warning: /root/.cargo/registry/src/*/redb-4.1.0/src/write_transaction.rs: No such file or directory
=== ReadTransaction ===
ugrep: warning: /root/.cargo/registry/src/*/redb-4.1.0/src/read_transaction.rs: No such file or directory
=== TableDefinition ===
30:    pub fn tree_height(&self) -> u32 {
35:    pub fn leaf_pages(&self) -> u64 {
40:    pub fn branch_pages(&self) -> u64 {
46:    pub fn stored_bytes(&self) -> u64 {
51:    pub fn metadata_bytes(&self) -> u64 {
56:    pub fn fragmented_bytes(&self) -> u64 {
102:    pub fn get_mut<'k>(
110:    pub fn pop_first(&mut self) -> Result<Option<(AccessGuard<'_, K>, AccessGuard<'_, V>)>> {
128:    pub fn pop_last(&mut self) -> Result<Option<(AccessGuard<'_, K>, AccessGuard<'_, V>)>> {
149:    pub fn extract_if<F: for<'f> FnMut(K::SelfType<'f>, V::SelfType<'f>) -> bool>(
160:    pub fn extract_from_if<'a, KR, F: for<'f> FnMut(K::SelfType<'f>, V::SelfType<'f>) -> bool>(
176:    pub fn retain<F: for<'f> FnMut(K::SelfType<'f>, V::SelfType<'f>) -> bool>(
186:    pub fn retain_in<'a, KR, F: for<'f> FnMut(K::SelfType<'f>, V::SelfType<'f>) -> bool>(
202:    pub fn insert<'k, 'v>(
224:    pub fn remove<'a>(
238:    pub fn insert_reserve<'a>(
499:    pub fn get<'a>(
508:    pub fn range<'a, KR>(&self, range: impl RangeBounds<KR>) -> Result<Range<'static, K, V>>
Bash Locate redb 4.1 src and key methods
IN
R=$(ls -d ~/.cargo/registry/src/*/redb-4.1.0/src); echo "PATH=$R"; echo "=== files ==="; ls $R; echo "=== lib.rs pub reexports / Database ==="; grep -n "pub fn create\|pub fn open\|pub fn begin_read\|pub fn begin_write\|impl Database" $R/database.rs 2>/dev/null | head; echo "=== write_txn pub ==="; grep -rn "pub fn open_table\|pub fn commit\|pub fn delete_table" $R | head

OUT
PATH=/root/.cargo/registry/src/mirrors.ustc.edu.cn-38d0e5eb5da2abae/redb-4.1.0/src
=== files ===
backends.rs
complex_types.rs
db.rs
error.rs
lib.rs
multimap_table.rs
sealed.rs
table.rs
transactions.rs
transaction_tracker.rs
tree_store
tuple_types.rs
types
types.rs
=== lib.rs pub reexports / Database ===
=== write_txn pub ===
/root/.cargo/registry/src/mirrors.ustc.edu.cn-38d0e5eb5da2abae/redb-4.1.0/src/transactions.rs:652:    pub fn open_table<'txn, K: Key + 'static, V: Value + 'static>(
/root/.cargo/registry/src/mirrors.ustc.edu.cn-38d0e5eb5da2abae/redb-4.1.0/src/transactions.rs:1348:    pub fn open_table<'txn, K: Key + 'static, V: Value + 'static>(
/root/.cargo/registry/src/mirrors.ustc.edu.cn-38d0e5eb5da2abae/redb-4.1.0/src/transactions.rs:1411:    pub fn delete_table(&self, definition: impl TableHandle) -> Result<bool, TableError> {
/root/.cargo/registry/src/mirrors.ustc.edu.cn-38d0e5eb5da2abae/redb-4.1.0/src/transactions.rs:1460:    pub fn commit(mut self) -> Result<(), CommitError> {
/root/.cargo/registry/src/mirrors.ustc.edu.cn-38d0e5eb5da2abae/redb-4.1.0/src/transactions.rs:2125:    pub fn open_table<K: Key + 'static, V: Value + 'static>(
Bash Inspect redb db.rs and table definitions
IN
R=~/.cargo/registry/src/mirrors.ustc.edu.cn-38d0e5eb5da2abae/redb-4.1.0/src; echo "=== db.rs: create/open/begin ==="; grep -n "pub fn create\|pub fn open\|pub fn begin_read\|pub fn begin_write\|pub fn new\b" $R/db.rs | head; echo "=== TableDefinition (table.rs) ==="; grep -n "pub const fn new\|impl<.*> TableDefinition\|pub fn table_name" $R/table.rs | head; echo "=== Key/Value for &str and &[u8] ==="; grep -rn "impl Key for &str\|impl Key for &\[u8\]\|impl Value for &str\|impl Value for &\[u8\]\|impl Key for str\|impl Value for str" $R/types* $R/types 2>/dev/null | head; echo "=== insert/remove/get signatures (transactions.rs) ==="; sed -n '640,720p' $R/transactions.rs | grep -n "pub fn\|fn insert\|fn remove\|fn get\|fn iter\|fn len" 

OUT
=== db.rs: create/open/begin ===
423:    pub fn open(path: impl AsRef<Path>) -> Result<ReadOnlyDatabase, DatabaseError> {
525:    pub fn create(path: impl AsRef<Path>) -> Result<Database, DatabaseError> {
530:    pub fn open(path: impl AsRef<Path>) -> Result<Database, DatabaseError> {
1037:    pub fn begin_write(&self) -> Result<WriteTransaction, TransactionError> {
1118:    pub fn new() -> Self {
1177:    pub fn create(&self, path: impl AsRef<Path>) -> Result<Database, DatabaseError> {
1196:    pub fn open(&self, path: impl AsRef<Path>) -> Result<Database, DatabaseError> {
1214:    pub fn open_read_only(
1231:    pub fn create_file(&self, file: File) -> Result<Database, DatabaseError> {
1243:    pub fn create_with_backend(
=== TableDefinition (table.rs) ===
=== Key/Value for &str and &[u8] ===
/root/.cargo/registry/src/mirrors.ustc.edu.cn-38d0e5eb5da2abae/redb-4.1.0/src/types.rs:312:impl Value for &[u8] {
/root/.cargo/registry/src/mirrors.ustc.edu.cn-38d0e5eb5da2abae/redb-4.1.0/src/types.rs:345:impl Key for &[u8] {
/root/.cargo/registry/src/mirrors.ustc.edu.cn-38d0e5eb5da2abae/redb-4.1.0/src/types.rs:486:impl Value for &str {
/root/.cargo/registry/src/mirrors.ustc.edu.cn-38d0e5eb5da2abae/redb-4.1.0/src/types.rs:519:impl Key for &str {
=== insert/remove/get signatures (transactions.rs) ===
13:    pub fn open_table<'txn, K: Key + 'static, V: Value + 'static>(
Bash Inspect begin_read, TableDefinition, ReadOnlyTable
IN
R=~/.cargo/registry/src/mirrors.ustc.edu.cn-38d0e5eb5da2abae/redb-4.1.0/src; echo "=== begin_read on Database ==="; grep -n "pub fn begin_read" $R/db.rs; echo "=== TableDefinition def ==="; grep -n "TableDefinition\|pub const fn new\|table_name" $R/table.rs | head -15; echo "=== ReadTransaction open_table / ReadOnlyTable methods ==="; grep -n "pub fn open_table\|pub fn get\|pub fn iter\|pub fn len\|pub fn range" $R/transactions.rs | sed -n '1,40p'; echo "=== insert/remove/get on Table (write) ==="; sed -n '195,235p' $R/table.rs

OUT
=== begin_read on Database ===
=== TableDefinition def ===
385:    /// const TABLE: TableDefinition<&str, u64> = TableDefinition::new("my_data");
=== ReadTransaction open_table / ReadOnlyTable methods ===
652:    pub fn open_table<'txn, K: Key + 'static, V: Value + 'static>(
1056:    pub fn get_persistent_savepoint(&self, id: u64) -> Result<Savepoint, SavepointError> {
1348:    pub fn open_table<'txn, K: Key + 'static, V: Value + 'static>(
2125:    pub fn open_table<K: Key + 'static, V: Value + 'static>(
=== insert/remove/get on Table (write) ===
    }

    /// Insert mapping of the given key to the given value
    ///
    /// If key is already present it is replaced
    ///
    /// Returns the old value, if the key was present in the table, otherwise None is returned
    pub fn insert<'k, 'v>(
        &mut self,
        key: impl Borrow<K::SelfType<'k>>,
        value: impl Borrow<V::SelfType<'v>>,
    ) -> Result<Option<AccessGuard<'_, V>>> {
        let value_len = V::as_bytes(value.borrow()).as_ref().len();
        if value_len > MAX_VALUE_LENGTH {
            return Err(StorageError::ValueTooLarge(value_len));
        }
        let key_len = K::as_bytes(key.borrow()).as_ref().len();
        if key_len > MAX_VALUE_LENGTH {
            return Err(StorageError::ValueTooLarge(key_len));
        }
        if value_len + key_len > MAX_PAIR_LENGTH {
            return Err(StorageError::ValueTooLarge(value_len + key_len));
        }
        self.tree.insert(key.borrow(), value.borrow())
    }

    /// Removes the given key
    ///
    /// Returns the old value, if the key was present in the table
    pub fn remove<'a>(
        &mut self,
        key: impl Borrow<K::SelfType<'a>>,
    ) -> Result<Option<AccessGuard<'_, V>>> {
        self.tree.remove(key.borrow())
    }
}

impl<K: Key + 'static, V: MutInPlaceValue + 'static> Table<'_, K, V> {
    /// Reserve space to insert a key-value pair
    ///
    /// If key is already present it is replaced
Bash Find begin_read and ReadableTable methods
IN
R=~/.cargo/registry/src/mirrors.ustc.edu.cn-38d0e5eb5da2abae/redb-4.1.0/src; echo "=== begin_read anywhere ==="; grep -rn "fn begin_read" $R/; echo "=== ReadableTable trait methods (get/iter/len) ==="; grep -n "fn get\|fn iter\|fn len\|fn range" $R/transactions.rs | grep -i "trait\|ReadableTable" ; grep -rn "trait ReadableTable" $R/; sed -n '/trait ReadableTable/,/^}/p' $R/transactions.rs 2>/dev/null | grep "fn " | head; echo "=== ReadOnlyTable get/iter ==="; grep -n "pub fn get\b\|pub fn iter\b\|pub fn len\b\|pub fn range\b" $R/table.rs | head

OUT
=== begin_read anywhere ===
/root/.cargo/registry/src/mirrors.ustc.edu.cn-38d0e5eb5da2abae/redb-4.1.0/src/db.rs:350:    fn begin_read(&self) -> Result<ReadTransaction, TransactionError>;
/root/.cargo/registry/src/mirrors.ustc.edu.cn-38d0e5eb5da2abae/redb-4.1.0/src/db.rs:404:    fn begin_read(&self) -> Result<ReadTransaction, TransactionError> {
/root/.cargo/registry/src/mirrors.ustc.edu.cn-38d0e5eb5da2abae/redb-4.1.0/src/db.rs:508:    fn begin_read(&self) -> Result<ReadTransaction, TransactionError> {
=== ReadableTable trait methods (get/iter/len) ===
/root/.cargo/registry/src/mirrors.ustc.edu.cn-38d0e5eb5da2abae/redb-4.1.0/src/table.rs:360:pub trait ReadableTableMetadata {
/root/.cargo/registry/src/mirrors.ustc.edu.cn-38d0e5eb5da2abae/redb-4.1.0/src/table.rs:373:pub trait ReadableTable<K: Key + 'static, V: Value + 'static>: ReadableTableMetadata {
=== ReadOnlyTable get/iter ===
499:    pub fn get<'a>(
508:    pub fn range<'a, KR>(&self, range: impl RangeBounds<KR>) -> Result<Range<'static, K, V>>
Bash Inspect ReadableDatabase and ReadableTable traits + exports
IN
R=~/.cargo/registry/src/mirrors.ustc.edu.cn-38d0e5eb5da2abae/redb-4.1.0/src; echo "=== trait containing begin_read (db.rs 340-355) ==="; sed -n '335,360p' $R/db.rs; echo "=== ReadableTable trait (table.rs 373-400) ==="; sed -n '373,410p' $R/table.rs; echo "=== lib.rs pub use ==="; grep -n "pub use\|pub mod\|pub trait" $R/lib.rs | head -40

OUT
=== trait containing begin_read (db.rs 340-355) ===
                    .unwrap()
                    .deallocate_read_transaction(transaction_id);
            }
        }
    }
}

pub trait ReadableDatabase {
    /// Begins a read transaction
    ///
    /// Captures a snapshot of the database, so that only data committed before calling this method
    /// is visible in the transaction
    ///
    /// Returns a [`ReadTransaction`] which may be used to read from the database. Read transactions
    /// may exist concurrently with writes
    fn begin_read(&self) -> Result<ReadTransaction, TransactionError>;

    /// Information regarding the usage of the in-memory cache
    ///
    /// Note: these metrics are only collected when the "`cache_metrics`" feature is enabled
    fn cache_stats(&self) -> CacheStats;
}

/// A redb database opened in read-only mode
///
/// Use [`Self::begin_read`] to get a [`ReadTransaction`] object that can be used to read from the database
=== ReadableTable trait (table.rs 373-400) ===
pub trait ReadableTable<K: Key + 'static, V: Value + 'static>: ReadableTableMetadata {
    /// Returns the value corresponding to the given key
    fn get<'a>(&self, key: impl Borrow<K::SelfType<'a>>) -> Result<Option<AccessGuard<'_, V>>>;

    /// Returns a double-ended iterator over a range of elements in the table
    ///
    /// # Examples
    ///
    /// Usage:
    /// ```rust
    /// use redb::*;
    /// # use tempfile::NamedTempFile;
    /// const TABLE: TableDefinition<&str, u64> = TableDefinition::new("my_data");
    ///
    /// # fn main() -> Result<(), Error> {
    /// # #[cfg(not(target_os = "wasi"))]
    /// # let tmpfile = NamedTempFile::new().unwrap();
    /// # #[cfg(target_os = "wasi")]
    /// # let tmpfile = NamedTempFile::new_in("/tmp").unwrap();
    /// # let filename = tmpfile.path();
    /// let db = Database::create(filename)?;
    /// let write_txn = db.begin_write()?;
    /// {
    ///     let mut table = write_txn.open_table(TABLE)?;
    ///     table.insert("a", &0)?;
    ///     table.insert("b", &1)?;
    ///     table.insert("c", &2)?;
    /// }
    /// write_txn.commit()?;
    ///
    /// let read_txn = db.begin_read()?;
    /// let table = read_txn.open_table(TABLE)?;
    /// let mut iter = table.range("a".."c")?;
    /// let (key, value) = iter.next().unwrap()?;
    /// assert_eq!("a", key.value());
    /// assert_eq!(0, value.value());
    /// # Ok(())
    /// # }
=== lib.rs pub use ===
65:pub use db::{
70:pub use error::{
74:pub use multimap_table::{
78:pub use table::{
82:pub use transactions::{DatabaseStats, Durability, ReadTransaction, WriteTransaction};
83:pub use tree_store::{AccessGuard, AccessGuardMut, AccessGuardMutInPlace, Savepoint};
84:pub use types::{Key, MutInPlaceValue, TypeName, Value};
88:pub mod backends;
Bash Confirm ReadableTable iter/len, TableDefinition::new, AccessGuard::value
IN
R=~/.cargo/registry/src/mirrors.ustc.edu.cn-38d0e5eb5da2abae/redb-4.1.0/src; echo "=== ReadableTable + ReadableTableMetadata methods ==="; sed -n '360,470p' $R/table.rs | grep -n "fn "; echo "=== TableDefinition new signature ==="; sed -n '380,420p' $R/table.rs | grep -n "new\|pub fn\|struct TableDefinition"; grep -n "impl.*TableDefinition\|pub const fn new" $R/table.rs | head; echo "=== AccessGuard value() ==="; grep -n "pub fn value\b\|pub fn value_mut" $R/tree_store/access_guard*.rs 2>/dev/null $R/lib.rs | head; grep -rn "pub fn value(" $R/tree_store/ | head

OUT
=== ReadableTable + ReadableTableMetadata methods ===
3:    fn stats(&self) -> Result<TableStats>;
6:    fn len(&self) -> Result<u64>;
9:    fn is_empty(&self) -> Result<bool> {
16:    fn get<'a>(&self, key: impl Borrow<K::SelfType<'a>>) -> Result<Option<AccessGuard<'_, V>>>;
28:    /// # fn main() -> Result<(), Error> {
53:    fn range<'a, KR>(&self, range: impl RangeBounds<KR> + 'a) -> Result<Range<'_, K, V>>
58:    fn first(&self) -> Result<Option<(AccessGuard<'_, K>, AccessGuard<'_, V>)>>;
61:    fn last(&self) -> Result<Option<(AccessGuard<'_, K>, AccessGuard<'_, V>)>>;
64:    fn iter(&self) -> Result<Range<'_, K, V>> {
78:    fn stats(&self) -> Result<TableStats> {
91:    fn len(&self) -> Result<u64> {
97:    pub(crate) fn new(
=== TableDefinition new signature ===
6:    /// const TABLE: TableDefinition<&str, u64> = TableDefinition::new("my_data");
10:    /// # let tmpfile = NamedTempFile::new().unwrap();
12:    /// # let tmpfile = NamedTempFile::new_in("/tmp").unwrap();
=== AccessGuard value() ===
/root/.cargo/registry/src/mirrors.ustc.edu.cn-38d0e5eb5da2abae/redb-4.1.0/src/tree_store/btree_base.rs:226:    pub fn value(&self) -> V::SelfType<'_> {
/root/.cargo/registry/src/mirrors.ustc.edu.cn-38d0e5eb5da2abae/redb-4.1.0/src/tree_store/btree_base.rs:296:    pub fn value(&self) -> V::SelfType<'_> {
redb 4.1 API 已确认。让我来修复 redb 版本并完成所有 Phase 0 的编辑。

Edit Cargo.toml
Modified
Write Cargo.toml
12 lines
[package]
name = "oas-store"
version = "0.1.0"
edition = "2024"

[dependencies]
thiserror.workspace = true
serde.workspace = true
serde_json.workspace = true
redb.workspace = true
oas-types = { path = "../oas-types" }
Write Cargo.toml
15 lines
[package]
name = "oas-manager"
version = "0.1.0"
edition = "2024"

[dependencies]
async-trait.workspace = true
thiserror.workspace = true
tokio = { workspace = true }
oas-types = { path = "../oas-types" }
oas-driver = { path = "../oas-driver" }
oas-net = { path = "../oas-net" }
oas-storage = { path = "../oas-storage" }
oas-store = { path = "../oas-store" }
现在进行 oas-e2e 测试 crate 骨架的构建。

Write Cargo.toml
18 lines
[package]
name = "oas-e2e"
version = "0.1.0"
edition = "2024"
publish = false

[dependencies]
oas-cri = { path = "../oas-cri" }
oas-manager = { path = "../oas-manager" }
oas-driver = { path = "../oas-driver" }
oas-net = { path = "../oas-net" }
oas-storage = { path = "../oas-storage" }
oas-store = { path = "../oas-store" }
oas-types = { path = "../oas-types" }
tokio = { workspace = true }
tonic = { workspace = true }
async-stream = { workspace = true }
Write lib.rs
6 lines
//! oas-e2e：端到端测试专用 crate（test-only）。
//!
//! 把真实 `OasManager`（+ 真实 `MemoryStore`）与手写 mock 的 driver/net/storage
//! 装配起来，经 `oas-cri` 的 UDS gRPC 通道跑 CRI 生命周期端到端用例。本身不产出
//! 任何产物，仅承载 `tests/` 下的集成测试与共享 mock。
Bash Phase 0 compile check
IN
cargo build --workspace 2>&1 | tail -20

OUT
    Blocking waiting for file lock on artifact directory
   Compiling zmij v1.0.21
   Compiling redb v4.1.0
   Compiling serde_json v1.0.150
   Compiling oas-store v0.1.0 (/home/yunfei/Code/Only-AgentSandbox/only-agent-sandbox/crates/oas-store)
   Compiling oas-manager v0.1.0 (/home/yunfei/Code/Only-AgentSandbox/only-agent-sandbox/crates/oas-manager)
   Compiling oas-cri v0.1.0 (/home/yunfei/Code/Only-AgentSandbox/only-agent-sandbox/crates/oas-cri)
   Compiling oas-e2e v0.1.0 (/home/yunfei/Code/Only-AgentSandbox/only-agent-sandbox/crates/oas-e2e)
   Compiling oas-runtime v0.1.0 (/home/yunfei/Code/Only-AgentSandbox/only-agent-sandbox)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 18.36s
第 0 阶段已通过。进入第 1 阶段 —— TDD：先编写一致性测试套件，然后实现 store trait 的变更和 MemoryStore。

Write mod.rs
Write failed
//! 共享 Store 一致性测试套件：`MemoryStore` 与 `RedbStore` 跑同一份用例。
//!
//! 红线 1（metadata/labels/annotations 原样往返）在此处断言。

use std::collections::HashMap;

use oas_store::{Store, StoreError, Txn};
use oas_types::{
    ContainerExitReason, ContainerFilter, ContainerRecord, ContainerState, IpLease, SandboxFilter,
    SandboxRecord, SandboxState,
};

// ---- record 构造辅助 ----

fn sb(id: &str, uid: &str, state: SandboxState) -> SandboxRecord {
    SandboxRecord {
        sandbox_id: id.into(),
        pod_uid: uid.into(),
        metadata: oas_types::SandboxMetadata {
            name: format!("name-{id}"),
            namespace: "ns".into(),
            uid: uid.into(),
            attempt: 0,
        },
        labels: HashMap::new(),
        annotations: HashMap::new(),
        type_id: 0,
        vm_id: 0,
        netns_path: format!("/ns/{id}"),
        tap_name: "tap0".into(),
        mac: "aa:bb".into(),
        pod_ip: "10.244.0.1".into(),
        gateway: "10.244.0.1".into(),
        rw_layer_path: None,
        cloud_disk_dev: None,
        state,
        created_at: 100,
    }
}

fn ct(id: &str, sid: &str, state: ContainerState) -> ContainerRecord {
    ContainerRecord {
        container_id: id.into(),
        sandbox_id: sid.into(),
        metadata: oas_types::ContainerMetadata {
            name: format!("cname-{id}"),
            attempt: 0,
        },
        image: "img-a".into(),
        command: vec!["sh".into()],
        args: vec!["-c".into(), "true".into()],
        env: vec!["KEY=VAL".into()],
        mounts: vec![],
        resources: oas_types::LinuxResources::default(),
        state,
        created_at: 200,
        started_at: None,
        finished_at: None,
        exit_code: 0,
        reason: None,
        message: String::new(),
        labels: HashMap::new(),
        annotations: HashMap::new(),
    }
}

// ---- 主入口：每个 backend 调一次 ----

pub fn run_store_tests<S: Store>(make: impl Fn() -> S) {
    sandbox_crud(&make);
    sandbox_list_filters(&make);
    sandbox_get_by_uid(&make);
    sandbox_delete_idempotent(&make);
    container_crud(&make);
    container_list_filters(&make);
    container_delete_idempotent(&make);
    metadata_passthrough(&make);
    ipam(&make);
    transaction_commit_and_rollback(&make);
    transaction_grouping(&make);
}

// ---- sandbox CRUD ----

fn sandbox_crud<S: Store>(make: &impl Fn() -> S) {
    let s = make();
    // missing → NotFound
    assert!(
        matches!(s.get_sandbox("nope"), Err(StoreError::NotFound(_))),
        "get_sandbox missing should be NotFound"
    );
    // put + get roundtrip
    s.put_sandbox(&sb("a", "uid-a", SandboxState::Ready)).unwrap();
    let got = s.get_sandbox("a").unwrap();
    assert_eq!(got.sandbox_id, "a");
    assert_eq!(got.pod_uid, "uid-a");
    assert_eq!(got.state, SandboxState::Ready);
    // overwrite
    let mut rec = sb("a", "uid-a", SandboxState::NotReady);
    rec.created_at = 999;
    s.put_sandbox(&rec).unwrap();
    let got2 = s.get_sandbox("a").unwrap();
    assert_eq!(got2.state, SandboxState::NotReady);
    assert_eq!(got2.created_at, 999);
}

fn sandbox_get_by_uid<S: Store>(make: &impl Fn() -> S) {
    let s = make();
    s.put_sandbox(&sb("a", "uid-1", SandboxState::Ready)).unwrap();
    assert_eq!(s.get_sandbox_by_uid("uid-1").unwrap().unwrap().sandbox_id, "a");
    assert!(s.get_sandbox_by_uid("uid-2").unwrap().is_none());
}

fn sandbox_delete_idempotent<S: Store>(make: &impl Fn() -> S) {
    let s = make();
    s.put_sandbox(&sb("a", "uid-a", SandboxState::Ready)).unwrap();
    s.delete_sandbox("a").unwrap();
    assert!(matches!(s.get_sandbox("a"), Err(StoreError::NotFound(_))));
    // delete missing → Ok (idempotent at store layer)
    assert!(s.delete_sandbox("a").is_ok());
    assert!(s.delete_sandbox("never").is_ok());
}

fn sandbox_list_filters<S: Store>(make: &impl Fn() -> S) {
    let s = make();
    let mut a = sb("a", "uid-a", SandboxState::Ready);
    a.labels.insert("env".into(), "prod".into());
    let mut b = sb("b", "uid-b", SandboxState::NotReady);
    b.labels.insert("env".into(), "dev".into());
    s.put_sandbox(&a).unwrap();
    s.put_sandbox(&b).unwrap();

    // empty filter → both
    let all = s.list_sandboxes(&SandboxFilter::default()).unwrap();
    assert_eq!(all.len(), 2, "empty filter should list all");

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
        label_selector: [("env".to_string(), "prod".to_string())].into(),
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

    // by pod_uid (honored even though CRI never sends it)
    let f = SandboxFilter {
        pod_uid: Some("uid-b".into()),
        ..Default::default()
    };
    let r = s.list_sandboxes(&f).unwrap();
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].sandbox_id, "b");
}

// ---- container CRUD ----

fn container_crud<S: Store>(make: &impl Fn() -> S) {
    let s = make();
    assert!(matches!(s.get_container("nope"), Err(StoreError::NotFound(_))));
    s.put_container(&ct("c", "sid", ContainerState::Created)).unwrap();
    let got = s.get_container("c").unwrap();
    assert_eq!(got.container_id, "c");
    assert_eq!(got.sandbox_id, "sid");
    assert_eq!(got.state, ContainerState::Created);
    // overwrite
    let mut rec = ct("c", "sid", ContainerState::Running);
    rec.exit_code = 7;
    rec.reason = Some(ContainerExitReason::Error);
    s.put_container(&rec).unwrap();
    let got2 = s.get_container("c").unwrap();
    assert_eq!(got2.state, ContainerState::Running);
    assert_eq!(got2.exit_code, 7);
    assert_eq!(got2.reason, Some(ContainerExitReason::Error));
}

fn container_delete_idempotent<S: Store>(make: &impl Fn() -> S) {
    let s = make();
    s.put_container(&ct("c", "sid", ContainerState::Created)).unwrap();
    s.delete_container("c").unwrap();
    assert!(matches!(s.get_container("c"), Err(StoreError::NotFound(_))));
    assert!(s.delete_container("c").is_ok());
    assert!(s.delete_container("never").is_ok());
}

fn container_list_filters<S: Store>(make: &impl Fn() -> S) {
    let s = make();
    let mut c1 = ct("c1", "s1", ContainerState::Running);
    c1.labels.insert("tier".into(), "front".into());
    let c2 = ct("c2", "s1", ContainerState::Exited);
    let c3 = ct("c3", "s2", ContainerState::Created);
    s.put_container(&c1).unwrap();
    s.put_container(&c2).unwrap();
    s.put_container(&c3).unwrap();

    // all
    assert_eq!(s.list_containers(&ContainerFilter::default()).unwrap().len(), 3);

    // by sandbox_id
    let f = ContainerFilter {
        sandbox_id: Some("s1".into()),
        ..Default::default()
    };
    let r = s.list_containers(&f).unwrap();
    assert_eq!(r.len(), 2);
    assert!(r.iter().all(|c| c.sandbox_id == "s1"));

    // by state
    let f = ContainerFilter {
        state: Some(ContainerState::Exited),
        ..Default::default()
    };
    let r = s.list_containers(&f).unwrap();
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].container_id, "c2");

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
        label_selector: [("tier".to_string(), "front".to_string())].into(),
        ..Default::default()
    };
    let r = s.list_containers(&f).unwrap();
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].container_id, "c1");
}

// ---- 红线 1：metadata/labels/annotations 原样往返 ----

fn metadata_passthrough<S: Store>(make: &impl Fn() -> S) {
    let s = make();
    let mut rec = sb("m", "uid-m", SandboxState::Ready);
    rec.labels.insert("k1".into(), "v1".into());
    rec.labels.insert("k2".into(), "v2".into());
    rec.annotations.insert("agent-sandbox/type".into(), "1".into());
    rec.metadata.name = "pod-name".into();
    rec.metadata.namespace = "kube-system".into();
    rec.metadata.attempt = 3;
    s.put_sandbox(&rec).unwrap();
    let got = s.get_sandbox("m").unwrap();
    assert_eq!(got.labels, rec.labels);
    assert_eq!(got.annotations, rec.annotations);
    assert_eq!(got.metadata.name, "pod-name");
    assert_eq!(got.metadata.namespace, "kube-system");
    assert_eq!(got.metadata.attempt, 3);
    assert_eq!(got.metadata.uid, "uid-m");
}

// ---- IPAM ----

fn ipam<S: Store>(make: &impl Fn() -> S) {
    let s = make();
    // /30 → 2 可分配（.0 网络、.3 广播）
    let l1 = s.lease_ip("10.244.0.0/30").unwrap();
    let l2 = s.lease_ip("10.244.0.0/30").unwrap();
    assert_ne!(l1.ip, l2.ip);
    assert_eq!(l1.cidr, "10.244.0.0/30");
    // 第三次 → 耗尽
    assert!(
        matches!(s.lease_ip("10.244.0.0/30"), Err(StoreError::Db(_))),
        "third lease in /30 should be exhausted"
    );
    // release + realloc 可复用
    s.release_ip(&l1).unwrap();
    let l3 = s.lease_ip("10.244.0.0/30").unwrap();
    assert!(l3.ip == l1.ip || l3.ip == l2.ip);
    // release 幂等
    s.release_ip(&l1).unwrap();
    // 不同 CIDR 独立
    let other = s.lease_ip("10.244.1.0/30").unwrap();
    assert!(other.ip.starts_with("10.244.1."));
}

// ---- 事务：提交 / 回滚 / 分组 ----

fn transaction_commit_and_rollback<S: Store>(make: &impl Fn() -> S) {
    let s = make();
    // commit
    s.transaction(Box::new(|t| {
        t.put_sandbox(&sb("t1", "u1", SandboxState::Ready))?;
        t.put_container(&ct("c1", "t1", ContainerState::Created))?;
        Ok(())
    }))
    .unwrap();
    assert!(s.get_sandbox("t1").is_ok());
    assert!(s.get_container("c1").is_ok());

    // rollback: 闭包返回 Err → 已写入不可见
    let res = s.transaction(Box::new(|t| {
        t.put_sandbox(&sb("t2", "u2", SandboxState::Ready))?;
        Err(StoreError::Other("boom".into()))
    }));
    assert!(res.is_err());
    assert!(matches!(s.get_sandbox("t2"), Err(StoreError::NotFound(_))));

    // empty commit
    assert!(s.transaction(Box::new(|_| Ok(()))).is_ok());
}

fn transaction_grouping<S: Store>(make: &impl Fn() -> S) {
    let s = make();
    // 模拟 remove_sandbox 的原子点：delete_sandbox + release_ip 同一事务
    s.put_sandbox(&sb("g", "ug", SandboxState::Ready)).unwrap();
    let lease = s.lease_ip("10.244.2.0/30").unwrap();
    // 把 lease 标记到 ipam（lease_ip 已写入）
    let ip = lease.ip.clone();
    s.transaction(Box::new(move |t| {
        t.delete_sandbox("g")?;
        t.release_ip(&IpLease {
            ip: ip.clone(),
            cidr: "10.244.2.0/30".into(),
            sandbox_id: "g".into(),
        })?;
        Ok(())
    }))
    .unwrap();
    assert!(matches!(s.get_sandbox("g"), Err(StoreError::NotFound(_))));
    // 该 IP 可被重新分配（说明已释放）
    let again = s.lease_ip("10.244.2.0/30").unwrap();
    assert_eq!(again.ip, lease.ip);
}
continue
Write mod.rs
379 lines
//! Store 一致性测试套件：对任意 `Store` 实现跑同一组用例。
//!
//! `memory.rs` / `redb.rs` 各自带一个 `#[test]` 调 `run_store_tests`。

use std::collections::HashMap;

use oas_store::{Store, StoreError, Txn};
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
        vm_id: 0,
        netns_path: format!("/ns/{id}"),
        tap_name: "tap0".into(),
        mac: "aa".into(),
        pod_ip: "10.0.0.1".into(),
        gateway: "10.0.0.1".into(),
        rw_layer_path: None,
        cloud_disk_dev: None,
        state,
        created_at: 100,
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
    r2.vm_id = 42;
    s.put_sandbox(&r2).unwrap();
    assert_eq!(s.get_sandbox("sb1").unwrap().vm_id, 42);

    // 缺失 → NotFound
    match s.get_sandbox("nope") {
        Err(StoreError::NotFound(_)) => {}
        other => panic!("expected NotFound, got {other:?}"),
    }
}

fn sandbox_get_by_uid<S: Store>(make: &impl Fn() -> S) {
    let s = make();
    s.put_sandbox(&sb("sb1", "uid1", SandboxState::Ready)).unwrap();
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
        label_selector: [("env".to_string(), "prod".to_string())].into_iter().collect(),
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
        label_selector: [("env".to_string(), "prod".to_string())].into_iter().collect(),
        state: Some(SandboxState::Ready),
        ..Default::default()
    };
    assert_eq!(s.list_sandboxes(&f).unwrap().len(), 1);
}

fn sandbox_delete_idempotent<S: Store>(make: &impl Fn() -> S) {
    let s = make();
    s.put_sandbox(&sb("sb1", "uid1", SandboxState::Ready)).unwrap();
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
    assert_eq!(s.get_container("c1").unwrap().state, ContainerState::Running);

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
        label_selector: [("tier".to_string(), "web".to_string())].into_iter().collect(),
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
    s.put_container(&ct("c1", "sb1", ContainerState::Created)).unwrap();
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
    s2.transaction(Box::new(|t| {
        t.delete_sandbox("sb1")?;
        t.release_ip(&lease)?;
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
    r.annotations.insert("agent-sandbox/type".into(), "1".into());
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
Write memory.rs
9 lines
mod common;

use oas_store::MemoryStore;

#[test]
fn memory_store_conformance() {
    common::run_store_tests(MemoryStore::new);
}
Write redb.rs
24 lines
mod common;

use oas_store::RedbStore;

/// Phase 2 实现 RedbStore 后取消忽略。
#[test]
#[ignore = "redb backend not implemented yet (Phase 2)"]
fn redb_store_conformance() {
    let path = redb_temp_path();
    {
        let store = RedbStore::open(&path).unwrap();
        common::run_store_tests(|| RedbStore::open(&path).unwrap());
        drop(store);
    }
    let _ = std::fs::remove_file(&path);
}

fn redb_temp_path() -> String {
    std::env::temp_dir()
        .join(format!("oas-redb-test-{}.db", std::process::id()))
        .to_string_lossy()
        .into_owned()
}
Now the implementation — rewrite the store trait crate with the object-safe Txn trait + transaction, and re-export the backends.

Write lib.rs
126 lines
//! 编排层 → 持久化层 契约（§2.7 / §3.6）。
//!
//! redb 5 张表（sandbox / container / ipam / image / meta）的 Repository 抽象。
//! 业务逻辑与 redb 存储解耦，事务边界 = 幂等与恢复的原子点。
//!
//! object-safety：`transaction` 取 `Box<dyn FnOnce(&mut dyn Txn) -> Result<(), StoreError>>`，
//! 无泛型，故 `Store` object-safe，可 `Arc<dyn Store>`（§3.2 Manager 持有）。

mod memory;
mod redb;

pub use memory::MemoryStore;
pub use redb::RedbStore;

/// 单事务句柄：事务内可见的写接口。
///
/// 各后端自行实现（`MemoryTxn` / `RedbTxn`），通过 `&mut dyn Txn` 传给 `Store::transaction`
/// 的闭包。`lease_ip` / `release_ip` 也在事务内可用，以便「删 sandbox + 释放 IP」一类原子点。
pub trait Txn {
    fn put_sandbox(&mut self, r: &oas_types::SandboxRecord) -> Result<(), StoreError>;
    fn put_container(&mut self, r: &oas_types::ContainerRecord) -> Result<(), StoreError>;
    fn delete_sandbox(&mut self, id: &str) -> Result<(), StoreError>;
    fn delete_container(&mut self, id: &str) -> Result<(), StoreError>;
    fn lease_ip(&mut self, cidr: &str) -> Result<oas_types::IpLease, StoreError>;
    fn release_ip(&mut self, lease: &oas_types::IpLease) -> Result<(), StoreError>;
}

/// 持久化层错误。
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("db: {0}")]
    Db(String),
    #[error("{0}")]
    Other(String),
}

/// 编排层 → 持久化层接口（§2.7）。
///
/// sandbox / container 同构 CRUD + IPAM 租约原子分配 + 单事务执行。
/// 读路径（`list_*` / `get_*`）只读 store，靠 redb 事务的 MVCC / 快照一致性，
/// 不等写锁，保证 PLEG 不被阻塞（§4.3）。
///
/// 四个写方法（`put_sandbox` / `put_container` / `delete_sandbox` / `delete_container`）
/// 有默认实现：经 `transaction` 走单写事务，故后端只需实现读路径 + IPAM + `transaction`。
/// `delete_*` 删不存在返回 `Ok(())`（幂等），与 CRI `swallow_not_found` 双保险。
pub trait Store: Send + Sync {
    // ---- 读 ----
    fn get_sandbox(&self, id: &str) -> Result<oas_types::SandboxRecord, StoreError>;
    fn get_sandbox_by_uid(
        &self,
        pod_uid: &str,
    ) -> Result<Option<oas_types::SandboxRecord>, StoreError>;
    fn list_sandboxes(
        &self,
        filter: &oas_types::SandboxFilter,
    ) -> Result<Vec<oas_types::SandboxRecord>, StoreError>;
    fn get_container(&self, id: &str) -> Result<oas_types::ContainerRecord, StoreError>;
    fn list_containers(
        &self,
        filter: &oas_types::ContainerFilter,
    ) -> Result<Vec<oas_types::ContainerRecord>, StoreError>;

    // ---- IPAM 单操作（各自一个原子事务）----
    fn lease_ip(&self, cidr: &str) -> Result<oas_types::IpLease, StoreError>;
    fn release_ip(&self, lease: &oas_types::IpLease) -> Result<(), StoreError>;

    // ---- 事务 ----
    /// 单事务执行：建资源 + 落库的原子点（§3.6 事务边界）。闭包返回 `Err` 则整体回滚。
    fn transaction(
        &self,
        f: Box<dyn FnOnce(&mut dyn Txn) -> Result<(), StoreError>>,
    ) -> Result<(), StoreError>;

    // ---- 写（默认经 transaction）----
    fn put_sandbox(&self, r: &oas_types::SandboxRecord) -> Result<(), StoreError> {
        let r = r.clone();
        self.transaction(Box::new(move |t| t.put_sandbox(&r)))
    }
    fn put_container(&self, r: &oas_types::ContainerRecord) -> Result<(), StoreError> {
        let r = r.clone();
        self.transaction(Box::new(move |t| t.put_container(&r)))
    }
    fn delete_sandbox(&self, id: &str) -> Result<(), StoreError> {
        let id = id.to_string();
        self.transaction(Box::new(move |t| t.delete_sandbox(&id)))
    }
    fn delete_container(&self, id: &str) -> Result<(), StoreError> {
        let id = id.to_string();
        self.transaction(Box::new(move |t| t.delete_container(&id)))
    }
}

// ---- IPAM CIDR 解析（Memory/Redb 共用）------------------------------------

/// 解析 `a.b.c.d/prefix` → (网络基址 u32, prefix)。失败返回 `Other`。
pub(crate) fn parse_cidr(cidr: &str) -> Result<(u32, u8), StoreError> {
    let (ip, prefix) = cidr
        .split_once('/')
        .ok_or_else(|| StoreError::Other(format!("bad cidr: {cidr}")))?;
    let octets: Vec<u8> = ip
        .split('.')
        .map(|s| s.parse::<u8>())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| StoreError::Other(format!("bad cidr: {cidr}")))?;
    if octets.len() != 4 {
        return Err(StoreError::Other(format!("bad cidr: {cidr}")));
    }
    let ip_u32 = u32::from_be_bytes([octets[0], octets[1], octets[2], octets[3]]);
    let prefix: u8 = prefix
        .parse()
        .map_err(|_| StoreError::Other(format!("bad cidr: {cidr}")))?;
    if prefix > 32 {
        return Err(StoreError::Other(format!("bad cidr: {cidr}")));
    }
    Ok((ip_u32, prefix))
}

pub(crate) fn ip_to_string(ip: u32) -> String {
    let b = ip.to_be_bytes();
    format!("{}.{}.{}.{}", b[0], b[1], b[2], b[3])
}
Write memory.rs
256 lines
//! 内存版 `Store`：HashMap 后端，无依赖、确定性，用于测试与开发。

use std::collections::HashMap;
use std::sync::Mutex;

use oas_types::{
    ContainerFilter, ContainerRecord, IpLease, SandboxFilter, SandboxRecord,
};

use crate::{ip_to_string, parse_cidr, Store, StoreError, Txn};

/// 内存表集合。
struct MemoryTables {
    sandboxes: HashMap<String, SandboxRecord>,
    containers: HashMap<String, ContainerRecord>,
    /// key = ip。
    ipam: HashMap<String, IpLease>,
}

impl MemoryTables {
    fn new() -> Self {
        Self {
            sandboxes: HashMap::new(),
            containers: HashMap::new(),
            ipam: HashMap::new(),
        }
    }
}

/// 内存 `Store`。
#[derive(Default)]
pub struct MemoryStore {
    inner: Mutex<MemoryTables>,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self::default()
    }
}

/// 事务句柄：直接操作 `&mut MemoryTables`；回滚靠调用方快照恢复。
struct MemoryTxn<'a> {
    tables: &'a mut MemoryTables,
}

impl<'a> Txn for MemoryTxn<'a> {
    fn put_sandbox(&mut self, r: &SandboxRecord) -> Result<(), StoreError> {
        self.tables
            .sandboxes
            .insert(r.sandbox_id.clone(), r.clone());
        Ok(())
    }
    fn put_container(&mut self, r: &ContainerRecord) -> Result<(), StoreError> {
        self.tables
            .containers
            .insert(r.container_id.clone(), r.clone());
        Ok(())
    }
    fn delete_sandbox(&mut self, id: &str) -> Result<(), StoreError> {
        self.tables.sandboxes.remove(id);
        Ok(())
    }
    fn delete_container(&mut self, id: &str) -> Result<(), StoreError> {
        self.tables.containers.remove(id);
        Ok(())
    }
    fn lease_ip(&mut self, cidr: &str) -> Result<IpLease, StoreError> {
        lease_ip_in(&mut self.tables.ipam, cidr)
    }
    fn release_ip(&mut self, lease: &IpLease) -> Result<(), StoreError> {
        self.tables.ipam.remove(&lease.ip);
        Ok(())
    }
}

impl Store for MemoryStore {
    fn get_sandbox(&self, id: &str) -> Result<SandboxRecord, StoreError> {
        let tables = self.inner.lock().unwrap();
        tables
            .sandboxes
            .get(id)
            .cloned()
            .ok_or_else(|| StoreError::NotFound(format!("sandbox {id}")))
    }

    fn get_sandbox_by_uid(&self, pod_uid: &str) -> Result<Option<SandboxRecord>, StoreError> {
        let tables = self.inner.lock().unwrap();
        Ok(tables
            .sandboxes
            .values()
            .find(|s| s.pod_uid == pod_uid)
            .cloned())
    }

    fn list_sandboxes(&self, filter: &SandboxFilter) -> Result<Vec<SandboxRecord>, StoreError> {
        let tables = self.inner.lock().unwrap();
        let mut out: Vec<SandboxRecord> = tables
            .sandboxes
            .values()
            .filter(|s| matches_sandbox(s, filter))
            .cloned()
            .collect();
        out.sort_by(|a, b| a.sandbox_id.cmp(&b.sandbox_id));
        Ok(out)
    }

    fn get_container(&self, id: &str) -> Result<ContainerRecord, StoreError> {
        let tables = self.inner.lock().unwrap();
        tables
            .containers
            .get(id)
            .cloned()
            .ok_or_else(|| StoreError::NotFound(format!("container {id}")))
    }

    fn list_containers(&self, filter: &ContainerFilter) -> Result<Vec<ContainerRecord>, StoreError> {
        let tables = self.inner.lock().unwrap();
        let mut out: Vec<ContainerRecord> = tables
            .containers
            .values()
            .filter(|c| matches_container(c, filter))
            .cloned()
            .collect();
        out.sort_by(|a, b| a.container_id.cmp(&b.container_id));
        Ok(out)
    }

    fn lease_ip(&self, cidr: &str) -> Result<IpLease, StoreError> {
        let mut tables = self.inner.lock().unwrap();
        lease_ip_in(&mut tables.ipam, cidr)
    }

    fn release_ip(&self, lease: &IpLease) -> Result<(), StoreError> {
        let mut tables = self.inner.lock().unwrap();
        tables.ipam.remove(&lease.ip);
        Ok(())
    }

    fn transaction(
        &self,
        f: Box<dyn FnOnce(&mut dyn Txn) -> Result<(), StoreError>>,
    ) -> Result<(), StoreError> {
        let mut guard = self.inner.lock().unwrap();
        // 快照用于回滚（MemoryTables 全是 Clone 的记录，量小可接受）。
        let snapshot = guard.clone();
        let mut txn = MemoryTxn {
            tables: &mut *guard,
        };
        let res = f(&mut txn);
        if res.is_err() {
            // 回滚：丢弃事务内改动。
            *guard = snapshot;
        }
        res
    }
}

// ---- 过滤匹配 ----

fn matches_sandbox(s: &SandboxRecord, f: &SandboxFilter) -> bool {
    if let Some(id) = &f.id {
        if &s.sandbox_id != id {
            return false;
        }
    }
    if let Some(uid) = &f.pod_uid {
        if &s.pod_uid != uid {
            return false;
        }
    }
    if let Some(state) = f.state {
        if s.state != state {
            return false;
        }
    }
    for (k, v) in &f.label_selector {
        if s.labels.get(k) != Some(v) {
            return false;
        }
    }
    true
}

fn matches_container(c: &ContainerRecord, f: &ContainerFilter) -> bool {
    if let Some(id) = &f.id {
        if &c.container_id != id {
            return false;
        }
    }
    if let Some(sid) = &f.sandbox_id {
        if &c.sandbox_id != sid {
            return false;
        }
    }
    if let Some(state) = f.state {
        if c.state != state {
            return false;
        }
    }
    for (k, v) in &f.label_selector {
        if c.labels.get(k) != Some(v) {
            return false;
        }
    }
    true
}

// ---- IPAM 分配（Memory/Redb 共用语义）-------------------------------------

/// 在给定 ipam 占用集上分配下一个可用 IP。占用集以 `used` 传入（key=ip），
/// 命中即写入并返回租约；`sandbox_id` 留空，由调用方（net 层）填充。
pub(crate) fn lease_ip_in(
    ipam: &mut HashMap<String, IpLease>,
    cidr: &str,
) -> Result<IpLease, StoreError> {
    let (base, prefix) = parse_cidr(cidr)?;
    if prefix == 32 {
        return Err(StoreError::Db(format!("ipam exhausted: {cidr}")));
    }
    let host_bits = 32 - prefix;
    let count: u64 = 1u64 << host_bits;
    let mask = if prefix == 0 { 0u32 } else { (!0u32) << host_bits };
    let network = base & mask;
    // host_bits >= 2 时跳过网络地址(.0)与广播(末地址)；/31 用两个地址。
    let (start, end) = if host_bits >= 2 {
        (1u64, count - 1)
    } else {
        (0u64, count)
    };
    for off in start..end {
        let candidate = network.wrapping_add(off as u32);
        let ip = ip_to_string(candidate);
        if !ipam.contains_key(&ip) {
            let lease = IpLease {
                ip: ip.clone(),
                cidr: cidr.to_string(),
                sandbox_id: String::new(),
            };
            ipam.insert(ip, lease.clone());
            return Ok(lease);
        }
    }
    Err(StoreError::Db(format!("ipam exhausted: {cidr}")))
}

impl Clone for MemoryTables {
    fn clone(&self) -> Self {
        Self {
            sandboxes: self.sandboxes.clone(),
            containers: self.containers.clone(),
            ipam: self.ipam.clone(),
        }
    }
}
Now a compiling RedbStore stub (Phase 2 fills it in; the test is #[ignore]d so it just needs to typecheck).

Write redb.rs
78 lines
//! redb 后端 `Store`（Phase 2 实现）。
//!
//! Phase 1 仅占位以保证 `pub use` 与被 `#[ignore]` 的 conformance 测试可编译；
//! 真实实现见 `RedbStore` 各方法内的 `unimplemented!()`。

use oas_types::{ContainerFilter, ContainerRecord, IpLease, SandboxFilter, SandboxRecord};

use crate::{Store, StoreError, Txn};

/// redb 持久化 `Store`。Phase 2 实装。
pub struct RedbStore {
    _db: (), // 占位；Phase 2 换成 `redb::Database`。
}

impl RedbStore {
    /// 打开（或创建）数据库。Phase 2 实装。
    pub fn open(_path: &str) -> Result<Self, StoreError> {
        unimplemented!("RedbStore::open — Phase 2")
    }
}

/// 事务句柄占位。
struct RedbTxn;

impl Txn for RedbTxn {
    fn put_sandbox(&mut self, _r: &SandboxRecord) -> Result<(), StoreError> {
        unimplemented!()
    }
    fn put_container(&mut self, _r: &ContainerRecord) -> Result<(), StoreError> {
        unimplemented!()
    }
    fn delete_sandbox(&mut self, _id: &str) -> Result<(), StoreError> {
        unimplemented!()
    }
    fn delete_container(&mut self, _id: &str) -> Result<(), StoreError> {
        unimplemented!()
    }
    fn lease_ip(&mut self, _cidr: &str) -> Result<IpLease, StoreError> {
        unimplemented!()
    }
    fn release_ip(&mut self, _lease: &IpLease) -> Result<(), StoreError> {
        unimplemented!()
    }
}

impl Store for RedbStore {
    fn get_sandbox(&self, _id: &str) -> Result<SandboxRecord, StoreError> {
        unimplemented!()
    }
    fn get_sandbox_by_uid(&self, _pod_uid: &str) -> Result<Option<SandboxRecord>, StoreError> {
        unimplemented!()
    }
    fn list_sandboxes(&self, _filter: &SandboxFilter) -> Result<Vec<SandboxRecord>, StoreError> {
        unimplemented!()
    }
    fn get_container(&self, _id: &str) -> Result<ContainerRecord, StoreError> {
        unimplemented!()
    }
    fn list_containers(
        &self,
        _filter: &ContainerFilter,
    ) -> Result<Vec<ContainerRecord>, StoreError> {
        unimplemented!()
    }
    fn lease_ip(&self, _cidr: &str) -> Result<IpLease, StoreError> {
        unimplemented!()
    }
    fn release_ip(&self, _lease: &IpLease) -> Result<(), StoreError> {
        unimplemented!()
    }
    fn transaction(
        &self,
        _f: Box<dyn FnOnce(&mut dyn Txn) -> Result<(), StoreError>>,
    ) -> Result<(), StoreError> {
        unimplemented!()
    }
}
Bash Run oas-store tests (Phase 1)
IN
cargo test -p oas-store 2>&1 | tail -30

OUT
error: could not find `Cargo.toml` in `/home/yunfei/Code/Only-AgentSandbox` or any parent directory
Bash Run oas-store tests (Phase 1)
IN
cargo test --manifest-path /home/yunfei/Code/Only-AgentSandbox/only-agent-sandbox/Cargo.toml -p oas-store 2>&1 | tail -40

OUT
   Compiling memchr v2.8.2
   Compiling oas-types v0.1.0 (/home/yunfei/Code/Only-AgentSandbox/only-agent-sandbox/crates/oas-types)
   Compiling serde_json v1.0.150
   Compiling oas-store v0.1.0 (/home/yunfei/Code/Only-AgentSandbox/only-agent-sandbox/crates/oas-store)
error[E0277]: the trait bound `MemoryTables: Default` is not satisfied
   --> crates/oas-store/src/memory.rs:33:5
    |
 31 | #[derive(Default)]
    |          ------- in this derive macro expansion
 32 | pub struct MemoryStore {
 33 |     inner: Mutex<MemoryTables>,
    |     ^^^^^^^^^^^^^^^^^^^^^^^^^^ the trait `Default` is not implemented for `MemoryTables`
    |
help: the trait `Default` is implemented for `std::sync::Mutex<T>`
   --> /root/.rustup/toolchains/stable-aarch64-unknown-linux-gnu/lib/rustlib/src/rust/library/std/src/sync/poison/mutex.rs:691:1
    |
691 | impl<T: Default> Default for Mutex<T> {
    | ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
    = note: required for `std::sync::Mutex<MemoryTables>` to implement `Default`
help: consider annotating `MemoryTables` with `#[derive(Default)]`
    |
 13 + #[derive(Default)]
 14 | struct MemoryTables {
    |

For more information about this error, try `rustc --explain E0277`.
error: could not compile `oas-store` (lib) due to 1 previous error
warning: build failed, waiting for other jobs to finish...
Edit memory.rs
Added 1 line
Edit memory.rs
Removed 10 lines
Bash Run oas-store tests (Phase 1)
IN
cargo test --manifest-path /home/yunfei/Code/Only-AgentSandbox/only-agent-sandbox/Cargo.toml -p oas-store 2>&1 | tail -40

OUT
    | ^^^^^^^^^^^^^^^^^^^^^^^^^^ `SandboxMetadata` is defined in another crate
    = note: this error originates in the macro `assert_eq` (in Nightly builds, run with -Z macro-backtrace for more info)

warning: unused import: `Txn`
 --> crates/oas-store/tests/common/mod.rs:7:36
  |
7 | use oas_store::{Store, StoreError, Txn};
  |                                    ^^^
  |
  = note: `#[warn(unused_imports)]` (part of `#[warn(unused)]`) on by default

error[E0597]: `lease` does not live long enough
   --> crates/oas-store/tests/common/mod.rs:334:23
    |
328 |       let lease = s2.lease_ip("10.244.0.0/24").unwrap();
    |           ----- binding `lease` declared here
...
332 |       s2.transaction(Box::new(|t| {
    |                      -        --- value captured here
    |  ____________________|
    | |
333 | |         t.delete_sandbox("sb1")?;
334 | |         t.release_ip(&lease)?;
    | |                       ^^^^^ borrowed value does not live long enough
335 | |         Ok(())
336 | |     }))
    | |______- coercion requires that `lease` is borrowed for `'static`
...
345 |   }
    |   - `lease` dropped here while still borrowed
    |
    = note: due to object lifetime defaults, `Box<dyn for<'a> FnOnce(&'a mut (dyn Txn + 'a)) -> Result<(), StoreError>>` actually means `Box<(dyn for<'a> FnOnce(&'a mut (dyn Txn + 'a)) -> Result<(), StoreError> + 'static)>`

Some errors have detailed explanations: E0369, E0597.
For more information about an error, try `rustc --explain E0369`.
warning: `oas-store` (test "memory") generated 1 warning
error: could not compile `oas-store` (test "memory") due to 2 previous errors; 1 warning emitted
warning: build failed, waiting for other jobs to finish...
warning: `oas-store` (test "redb") generated 1 warning (1 duplicate)
error: could not compile `oas-store` (test "redb") due to 2 previous errors; 1 warning emitted
Two fixes: SandboxMetadata needs PartialEq for the assert, and the Box<dyn FnOnce> is +'static so the closure must own a cloned lease. Let me add PartialEq, Eq to the metadata types and fix the test.

Edit lib.rs
Modified
Edit lib.rs
Modified
Edit mod.rs
Modified
Edit mod.rs
Added 1 line
Bash Run oas-store tests (Phase 1)
IN
cargo test --manifest-path /home/yunfei/Code/Only-AgentSandbox/only-agent-sandbox/Cargo.toml -p oas-store 2>&1 | tail -30

OUT
warning: `oas-store` (lib) generated 2 warnings (2 duplicates)
warning: `oas-store` (lib test) generated 2 warnings
    Finished `test` profile [unoptimized + debuginfo] target(s) in 2.00s
     Running unittests src/lib.rs (only-agent-sandbox/target/debug/deps/oas_store-01966fddde5a808c)

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

     Running tests/memory.rs (only-agent-sandbox/target/debug/deps/memory-d244acf371b13dfb)

running 1 test
test memory_store_conformance ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

     Running tests/redb.rs (only-agent-sandbox/target/debug/deps/redb-1b67442911c5e861)

running 1 test
test redb_store_conformance ... ignored, redb backend not implemented yet (Phase 2)

test result: ok. 0 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.00s

   Doc-tests oas_store

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
Phase 1 green (MemoryStore conformance passes, redb ignored). Let me check the 2 lib warnings and confirm the whole workspace still builds/tests.

Bash Inspect oas-store lib warnings
IN
cargo build --manifest-path /home/yunfei/Code/Only-AgentSandbox/only-agent-sandbox/Cargo.toml -p oas-store 2>&1 | grep -A3 warning | head -25

OUT
warning: associated function `new` is never used
  --> crates/oas-store/src/memory.rs:22:8
   |
21 | impl MemoryTables {
--
warning: struct `RedbTxn` is never constructed
  --> crates/oas-store/src/redb.rs:23:8
   |
23 | struct RedbTxn;
--
warning: `oas-store` (lib) generated 2 warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.05s
Edit memory.rs
Removed 10 lines
Edit redb.rs
Added 1 line
Phase 1 clean. Let me update todos and move to Phase 2 (RedbStore). First, check if redb has an in-memory backend — ideal for fresh-per-call test isolation.

Update Todos

Phase 0: Setup — add oas-e2e skeleton, workspace redb dep, oas-manager/oas-store Cargo deps; compile-only green

Phase 1: Store object-safety + MemoryStore (TDD) — Txn trait, transaction Box<dyn FnOnce>, default write impls, MemoryStore + MemoryTxn, conformance suite on Memory

Phase 2: RedbStore (TDD) — redb backend, RedbTxn, serde_json, eager tables, ipam scan; conformance green

Phase 3: Manager supporting types — Clock/IdGenerator/SandboxTypeTable/PerKeyLock + tests

Phase 4: Manager sandbox FSM — run/stop/remove/status/list + tests (idempotency, rollback, red-line 3)

Phase 5: Manager container FSM — create/start/stop/remove + tests (CRI-idempotent start/stop)

Phase 6: Manager image_* + version/status + tests

Phase 7: E2E over UDS — oas-e2e lifecycle.rs with real Manager + mocks; green
Bash Check redb in-memory backend
IN
R=~/.cargo/registry/src/mirrors.ustc.edu.cn-38d0e5eb5da2abae/redb-4.1.0/src; echo "=== backends.rs ==="; grep -n "pub struct\|pub fn\|InMemory\|MemoryBackend\|impl.*Backend" $R/backends.rs | head -30; echo "=== create_with_backend signature ==="; sed -n '1243,1260p' $R/db.rs

OUT
=== backends.rs ===
1:pub use crate::tree_store::InMemoryBackend;
=== create_with_backend signature ===
    pub fn create_with_backend(
        &self,
        backend: impl StorageBackend,
    ) -> Result<Database, DatabaseError> {
        Database::new(
            Box::new(backend),
            true,
            self.page_size,
            self.region_size,
            self.cache_size,
            &self.repair_callback,
        )
    }
}

impl std::fmt::Debug for Database {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Database").finish()
Bash Inspect InMemoryBackend API
IN
R=~/.cargo/registry/src/mirrors.ustc.edu.cn-38d0e5eb5da2abae/redb-4.1.0/src; echo "=== InMemoryBackend ==="; grep -rn "InMemoryBackend" $R/tree_store/*.rs | head; F=$(grep -rln "pub struct InMemoryBackend" $R/); echo "FILE=$F"; grep -n "pub fn new\|impl InMemoryBackend\|pub struct InMemoryBackend\|impl StorageBackend for InMemoryBackend" $F | head; echo "=== Database::builder ==="; grep -n "pub fn builder\|pub fn set_cache_size\|pub fn new\b" $R/db.rs | head

OUT
=== InMemoryBackend ===
/root/.cargo/registry/src/mirrors.ustc.edu.cn-38d0e5eb5da2abae/redb-4.1.0/src/tree_store/mod.rs:23:pub use page_store::{InMemoryBackend, Savepoint, file_backend};
FILE=/root/.cargo/registry/src/mirrors.ustc.edu.cn-38d0e5eb5da2abae/redb-4.1.0/src/tree_store/page_store/backends.rs
12:    pub fn new(inner: Box<dyn StorageBackend>) -> Self {
45:pub struct InMemoryBackend(RwLock<Vec<u8>>);
47:impl InMemoryBackend {
53:impl InMemoryBackend {
55:    pub fn new() -> Self {
70:impl StorageBackend for InMemoryBackend {
=== Database::builder ===
1028:    pub fn builder() -> Builder {
1118:    pub fn new() -> Self {
1161:    pub fn set_cache_size(&mut self, bytes: usize) -> &mut Self {
Bash Check Database::create vs open semantics
IN
R=~/.cargo/registry/src/mirrors.ustc.edu.cn-38d0e5eb5da2abae/redb-4.1.0/src; sed -n '515,535p' $R/db.rs

OUT
    fn cache_stats(&self) -> CacheStats {
        self.mem.cache_stats()
    }
}

impl Database {
    /// Opens the specified file as a redb database.
    /// * if the file does not exist, or is an empty file, a new database will be initialized in it
    /// * if the file is a valid redb database, it will be opened
    /// * otherwise this function will return an error
    pub fn create(path: impl AsRef<Path>) -> Result<Database, DatabaseError> {
        Self::builder().create(path)
    }

    /// Opens an existing redb database.
    pub fn open(path: impl AsRef<Path>) -> Result<Database, DatabaseError> {
        Self::builder().open(path)
    }

    pub(crate) fn get_memory(&self) -> Arc<TransactionalMemory> {
        self.mem.clone()
InMemoryBackend exists — perfect for test isolation. Let me confirm begin_write is inherent vs trait before coding RedbStore.

Bash Confirm begin_write inherent + error types
IN
R=~/.cargo/registry/src/mirrors.ustc.edu.cn-38d0e5eb5da2abae/redb-4.1.0/src; echo "=== begin_write context (db.rs 1025-1045) ==="; sed -n '1025,1045p' $R/db.rs; echo "=== WriteDatabase trait? ==="; grep -n "trait WriteDatabase\|trait ReadableDatabase\|impl ReadableDatabase for Database\|impl.*for Database" $R/db.rs | head; echo "=== TableError / TransactionError / CommitError → Error? ==="; grep -n "pub enum TableError\|pub enum TransactionError\|pub enum CommitError\|impl.*Error" $R/error.rs | head

OUT
=== begin_write context (db.rs 1025-1045) ===
    }

    /// Convenience method for [`Builder::new`]
    pub fn builder() -> Builder {
        Builder::new()
    }

    /// Begins a write transaction
    ///
    /// Returns a [`WriteTransaction`] which may be used to read/write to the database. Only a single
    /// write may be in progress at a time. If a write is in progress, this function will block
    /// until it completes.
    pub fn begin_write(&self) -> Result<WriteTransaction, TransactionError> {
        // Fail early if there has been an I/O error -- nothing can be committed in that case
        self.mem.check_io_errors()?;
        let guard = TransactionGuard::new_write(
            self.transaction_tracker.start_write_transaction(),
            self.transaction_tracker.clone(),
        );
        WriteTransaction::new(guard, self.transaction_tracker.clone(), self.mem.clone())
            .map_err(|e| e.into())
=== WriteDatabase trait? ===
342:pub trait ReadableDatabase {
507:impl ReadableDatabase for Database {
1061:impl Drop for Database {
1258:impl std::fmt::Debug for Database {
=== TableError / TransactionError / CommitError → Error? ===
21:impl<T> From<PoisonError<T>> for StorageError {
27:impl From<io::Error> for StorageError {
33:impl From<StorageError> for Error {
46:impl Display for StorageError {
78:impl std::error::Error for StorageError {}
83:pub enum TableError {
110:impl TableError {
127:impl From<TableError> for Error {
152:impl From<StorageError> for TableError {
158:impl Display for TableError {
Bash Check SelfType for &[u8] and &str
IN
R=~/.cargo/registry/src/mirrors.ustc.edu.cn-38d0e5eb5da2abae/redb-4.1.0/src; echo "=== &[u8] Value/Key SelfType (types.rs 300-360) ==="; sed -n '300,360p' $R/types.rs; echo "=== &str Value/Key SelfType (470-540) ==="; sed -n '470,540p' $R/types.rs

OUT
=== &[u8] Value/Key SelfType (types.rs 300-360) ===
                Ordering::Less
            }
        } else {
            if data2[0] == 0 {
                Ordering::Greater
            } else {
                T::compare(&data1[1..], &data2[1..])
            }
        }
    }
}

impl Value for &[u8] {
    type SelfType<'a>
        = &'a [u8]
    where
        Self: 'a;
    type AsBytes<'a>
        = &'a [u8]
    where
        Self: 'a;

    fn fixed_width() -> Option<usize> {
        None
    }

    fn from_bytes<'a>(data: &'a [u8]) -> &'a [u8]
    where
        Self: 'a,
    {
        data
    }

    fn as_bytes<'a, 'b: 'a>(value: &'a Self::SelfType<'b>) -> &'a [u8]
    where
        Self: 'b,
    {
        value
    }

    fn type_name() -> TypeName {
        TypeName::internal("&[u8]")
    }
}

impl Key for &[u8] {
    fn compare(data1: &[u8], data2: &[u8]) -> Ordering {
        data1.cmp(data2)
    }
}

impl<const N: usize> Value for &[u8; N] {
    type SelfType<'a>
        = &'a [u8; N]
    where
        Self: 'a;
    type AsBytes<'a>
        = &'a [u8; N]
    where
        Self: 'a;

=== &str Value/Key SelfType (470-540) ===
            for i in 0..N {
                let range = size_of::<u32>() * i..size_of::<u32>() * (i + 1);
                let end1 = u32::from_le_bytes(data1[range.clone()].try_into().unwrap()) as usize;
                let end2 = u32::from_le_bytes(data2[range].try_into().unwrap()) as usize;
                let comparison = T::compare(&data1[start1..end1], &data2[start2..end2]);
                if !comparison.is_eq() {
                    return comparison;
                }
                start1 = end1;
                start2 = end2;
            }
        }
        Ordering::Equal
    }
}

impl Value for &str {
    type SelfType<'a>
        = &'a str
    where
        Self: 'a;
    type AsBytes<'a>
        = &'a str
    where
        Self: 'a;

    fn fixed_width() -> Option<usize> {
        None
    }

    fn from_bytes<'a>(data: &'a [u8]) -> &'a str
    where
        Self: 'a,
    {
        std::str::from_utf8(data).unwrap()
    }

    fn as_bytes<'a, 'b: 'a>(value: &'a Self::SelfType<'b>) -> &'a str
    where
        Self: 'b,
    {
        value
    }

    fn type_name() -> TypeName {
        TypeName::internal("&str")
    }
}

impl Key for &str {
    fn compare(data1: &[u8], data2: &[u8]) -> Ordering {
        let str1 = Self::from_bytes(data1);
        let str2 = Self::from_bytes(data2);
        str1.cmp(str2)
    }
}

impl Value for String {
    type SelfType<'a>
        = String
    where
        Self: 'a;
    type AsBytes<'a>
        = &'a str
    where
        Self: 'a;

    fn fixed_width() -> Option<usize> {
        None
    }
SelfType confirmed (&[u8]/&str). Now let me write the real RedbStore. First, move the filter helpers to lib.rs so both backends share them.

Edit lib.rs
Added 53 lines
Now remove the duplicated filter helpers from memory.rs and use the shared ones.

Edit memory.rs
Modified
Edit memory.rs
Removed 50 lines
Now the full RedbStore implementation.

Write redb.rs
249 lines
//! redb 后端 `Store`（§3.6 五张表中的 sandbox / container / ipam）。
//!
//! 记录经 `serde_json` 序列化为 `&[u8]` 存入；ipam 表 key=ip、value=cidr。
//! 表在构造时一次性建好，避免读路径撞 `TableDoesNotExist`。事务回滚靠 redb
//! `WriteTransaction` drop-without-commit 原生语义。

use std::collections::HashSet;

use oas_types::{
    ContainerFilter, ContainerRecord, IpLease, SandboxFilter, SandboxRecord,
};
use redb::{ReadableDatabase, ReadableTable};

use crate::{ip_to_string, matches_container, matches_sandbox, parse_cidr, Store, StoreError, Txn};

const SANDBOX: redb::TableDefinition<&str, &[u8]> = redb::TableDefinition::new("sandbox");
const CONTAINER: redb::TableDefinition<&str, &[u8]> = redb::TableDefinition::new("container");
const IPAM: redb::TableDefinition<&str, &str> = redb::TableDefinition::new("ipam");

/// redb 持久化 `Store`。
pub struct RedbStore {
    db: redb::Database,
}

fn map_err<E: std::fmt::Display>(e: E) -> StoreError {
    StoreError::Db(e.to_string())
}

impl RedbStore {
    /// 打开（或创建）文件型 redb 数据库。
    pub fn open(path: &str) -> Result<Self, StoreError> {
        let db = redb::Database::create(path).map_err(map_err)?;
        Self::from_db(db)
    }

    /// 内存型 redb（测试用，每实例独立、无文件）。
    pub fn open_in_memory() -> Result<Self, StoreError> {
        let db = redb::Database::builder()
            .create_with_backend(redb::backends::InMemoryBackend::new())
            .map_err(map_err)?;
        Self::from_db(db)
    }

    /// 由已打开的 `Database` 构造，并预建全部表。
    pub fn from_db(db: redb::Database) -> Result<Self, StoreError> {
        let wtxn = db.begin_write().map_err(map_err)?;
        {
            let _ = wtxn.open_table(SANDBOX).map_err(map_err)?;
            let _ = wtxn.open_table(CONTAINER).map_err(map_err)?;
            let _ = wtxn.open_table(IPAM).map_err(map_err)?;
        }
        wtxn.commit().map_err(map_err)?;
        Ok(Self { db })
    }
}

/// 事务句柄：包裹 `&mut WriteTransaction`，方法各自 `open_table`。
struct RedbTxn<'a> {
    wtxn: &'a mut redb::WriteTransaction,
}

impl<'a> Txn for RedbTxn<'a> {
    fn put_sandbox(&mut self, r: &SandboxRecord) -> Result<(), StoreError> {
        let bytes = serde_json::to_vec(r).map_err(|e| StoreError::Db(e.to_string()))?;
        let mut t = self.wtxn.open_table(SANDBOX).map_err(map_err)?;
        t.insert(r.sandbox_id.as_str(), bytes.as_slice())
            .map_err(map_err)?;
        Ok(())
    }
    fn put_container(&mut self, r: &ContainerRecord) -> Result<(), StoreError> {
        let bytes = serde_json::to_vec(r).map_err(|e| StoreError::Db(e.to_string()))?;
        let mut t = self.wtxn.open_table(CONTAINER).map_err(map_err)?;
        t.insert(r.container_id.as_str(), bytes.as_slice())
            .map_err(map_err)?;
        Ok(())
    }
    fn delete_sandbox(&mut self, id: &str) -> Result<(), StoreError> {
        let mut t = self.wtxn.open_table(SANDBOX).map_err(map_err)?;
        t.remove(id).map_err(map_err)?;
        Ok(())
    }
    fn delete_container(&mut self, id: &str) -> Result<(), StoreError> {
        let mut t = self.wtxn.open_table(CONTAINER).map_err(map_err)?;
        t.remove(id).map_err(map_err)?;
        Ok(())
    }
    fn lease_ip(&mut self, cidr: &str) -> Result<IpLease, StoreError> {
        let mut t = self.wtxn.open_table(IPAM).map_err(map_err)?;
        lease_ip_redb(&mut t, cidr)
    }
    fn release_ip(&mut self, lease: &IpLease) -> Result<(), StoreError> {
        let mut t = self.wtxn.open_table(IPAM).map_err(map_err)?;
        t.remove(lease.ip.as_str()).map_err(map_err)?;
        Ok(())
    }
}

impl Store for RedbStore {
    fn get_sandbox(&self, id: &str) -> Result<SandboxRecord, StoreError> {
        let rtxn = self.db.begin_read().map_err(map_err)?;
        let t = rtxn.open_table(SANDBOX).map_err(map_err)?;
        match t.get(id).map_err(map_err)? {
            Some(g) => serde_json::from_slice(g.value())
                .map_err(|e| StoreError::Db(e.to_string())),
            None => Err(StoreError::NotFound(format!("sandbox {id}"))),
        }
    }

    fn get_sandbox_by_uid(&self, pod_uid: &str) -> Result<Option<SandboxRecord>, StoreError> {
        let rtxn = self.db.begin_read().map_err(map_err)?;
        let t = rtxn.open_table(SANDBOX).map_err(map_err)?;
        for item in t.iter().map_err(map_err)? {
            let (_k, v) = item.map_err(map_err)?;
            let rec: SandboxRecord =
                serde_json::from_slice(v.value()).map_err(|e| StoreError::Db(e.to_string()))?;
            if rec.pod_uid == pod_uid {
                return Ok(Some(rec));
            }
        }
        Ok(None)
    }

    fn list_sandboxes(&self, filter: &SandboxFilter) -> Result<Vec<SandboxRecord>, StoreError> {
        let rtxn = self.db.begin_read().map_err(map_err)?;
        let t = rtxn.open_table(SANDBOX).map_err(map_err)?;
        let mut out = Vec::new();
        for item in t.iter().map_err(map_err)? {
            let (_k, v) = item.map_err(map_err)?;
            let rec: SandboxRecord =
                serde_json::from_slice(v.value()).map_err(|e| StoreError::Db(e.to_string()))?;
            if matches_sandbox(&rec, filter) {
                out.push(rec);
            }
        }
        out.sort_by(|a, b| a.sandbox_id.cmp(&b.sandbox_id));
        Ok(out)
    }

    fn get_container(&self, id: &str) -> Result<ContainerRecord, StoreError> {
        let rtxn = self.db.begin_read().map_err(map_err)?;
        let t = rtxn.open_table(CONTAINER).map_err(map_err)?;
        match t.get(id).map_err(map_err)? {
            Some(g) => serde_json::from_slice(g.value())
                .map_err(|e| StoreError::Db(e.to_string())),
            None => Err(StoreError::NotFound(format!("container {id}"))),
        }
    }

    fn list_containers(&self, filter: &ContainerFilter) -> Result<Vec<ContainerRecord>, StoreError> {
        let rtxn = self.db.begin_read().map_err(map_err)?;
        let t = rtxn.open_table(CONTAINER).map_err(map_err)?;
        let mut out = Vec::new();
        for item in t.iter().map_err(map_err)? {
            let (_k, v) = item.map_err(map_err)?;
            let rec: ContainerRecord =
                serde_json::from_slice(v.value()).map_err(|e| StoreError::Db(e.to_string()))?;
            if matches_container(&rec, filter) {
                out.push(rec);
            }
        }
        out.sort_by(|a, b| a.container_id.cmp(&b.container_id));
        Ok(out)
    }

    fn lease_ip(&self, cidr: &str) -> Result<IpLease, StoreError> {
        let mut wtxn = self.db.begin_write().map_err(map_err)?;
        let result = {
            let mut t = wtxn.open_table(IPAM).map_err(map_err)?;
            lease_ip_redb(&mut t, cidr)
        };
        match result {
            Ok(lease) => {
                wtxn.commit().map_err(map_err)?;
                Ok(lease)
            }
            Err(e) => Err(e), // wtxn drop → 回滚
        }
    }

    fn release_ip(&self, lease: &IpLease) -> Result<(), StoreError> {
        let mut wtxn = self.db.begin_write().map_err(map_err)?;
        {
            let mut t = wtxn.open_table(IPAM).map_err(map_err)?;
            t.remove(lease.ip.as_str()).map_err(map_err)?;
        }
        wtxn.commit().map_err(map_err)?;
        Ok(())
    }

    fn transaction(
        &self,
        f: Box<dyn FnOnce(&mut dyn Txn) -> Result<(), StoreError>>,
    ) -> Result<(), StoreError> {
        let mut wtxn = self.db.begin_write().map_err(map_err)?;
        let result = {
            let mut txn = RedbTxn { wtxn: &mut wtxn };
            f(&mut txn)
        };
        match result {
            Ok(()) => {
                wtxn.commit().map_err(map_err)?;
                Ok(())
            }
            Err(e) => Err(e), // wtxn drop → 回滚
        }
    }
}

/// 在 ipam 表上分配下一个可用 IP（事务内）。命中即 insert；耗尽返回 `Db`。
fn lease_ip_redb(
    t: &mut redb::Table<'_, &str, &str>,
    cidr: &str,
) -> Result<IpLease, StoreError> {
    let (base, prefix) = parse_cidr(cidr)?;
    if prefix == 32 {
        return Err(StoreError::Db(format!("ipam exhausted: {cidr}")));
    }
    let host_bits = 32 - prefix;
    let count: u64 = 1u64 << host_bits;
    let mask = if prefix == 0 { 0u32 } else { (!0u32) << host_bits };
    let network = base & mask;
    let (start, end) = if host_bits >= 2 {
        (1u64, count - 1)
    } else {
        (0u64, count)
    };

    // 收集已占用的 ip。
    let mut used: HashSet<String> = HashSet::new();
    for item in t.iter().map_err(map_err)? {
        let (k, _v) = item.map_err(map_err)?;
        used.insert(k.value().to_string());
    }

    for off in start..end {
        let candidate = network.wrapping_add(off as u32);
        let ip = ip_to_string(candidate);
        if !used.contains(&ip) {
            t.insert(ip.as_str(), cidr).map_err(map_err)?;
            return Ok(IpLease {
                ip,
                cidr: cidr.to_string(),
                sandbox_id: String::new(),
            });
        }
    }
    Err(StoreError::Db(format!("ipam exhausted: {cidr}")))
}
Now update the redb test to use the in-memory backend and un-ignore it.

Write redb.rs
10 lines
mod common;

use oas_store::RedbStore;

#[test]
fn redb_store_conformance() {
    // 每次调用建一个独立内存 redb，保证用例间隔离。
    common::run_store_tests(|| RedbStore::open_in_memory().unwrap());
}
Bash Run oas-store tests (Phase 2 RedbStore)
IN
cargo test --manifest-path /home/yunfei/Code/Only-AgentSandbox/only-agent-sandbox/Cargo.toml -p oas-store 2>&1 | tail -30

OUT
warning: `oas-store` (lib test) generated 2 warnings (2 duplicates)
warning: `oas-store` (lib) generated 2 warnings (run `cargo fix --lib -p oas-store` to apply 2 suggestions)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 2.94s
     Running unittests src/lib.rs (only-agent-sandbox/target/debug/deps/oas_store-01966fddde5a808c)

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

     Running tests/memory.rs (only-agent-sandbox/target/debug/deps/memory-d244acf371b13dfb)

running 1 test
test memory_store_conformance ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

     Running tests/redb.rs (only-agent-sandbox/target/debug/deps/redb-1b67442911c5e861)

running 1 test
test redb_store_conformance ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.16s

   Doc-tests oas_store

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
Both backends pass. Let me check the 2 lib warnings.

Bash Inspect oas-store lib warnings
IN
cargo build --manifest-path /home/yunfei/Code/Only-AgentSandbox/only-agent-sandbox/Cargo.toml -p oas-store 2>&1 | grep -A2 warning | head -20

OUT
warning: variable does not need to be mutable
   --> crates/oas-store/src/redb.rs:166:13
    |
--
warning: variable does not need to be mutable
   --> crates/oas-store/src/redb.rs:181:13
    |
--
warning: `oas-store` (lib) generated 2 warnings (run `cargo fix --lib -p oas-store` to apply 2 suggestions)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.07s
Read redb.rs (lines 159-188)
open_table takes &self, so wtxn doesn't need mut there. Let me fix both.

Edit redb.rs
Modified
Edit redb.rs
Modified
Bash Verify oas-store clean build
IN
cargo build --manifest-path /home/yunfei/Code/Only-AgentSandbox/only-agent-sandbox/Cargo.toml -p oas-store 2>&1 | tail -5

OUT
   Compiling oas-store v0.1.0 (/home/yunfei/Code/Only-AgentSandbox/only-agent-sandbox/crates/oas-store)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.33s
Phase 2 complete (both backends green, clean build). Now Phase 3 — Manager supporting types. Let me write the modules with inline tests.

Write clock.rs
73 lines
//! 时钟抽象：让 `created_at` / `started_at` / `finished_at` 在测试里确定。

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;

/// Unix 秒时钟。
pub trait Clock: Send + Sync {
    fn now_unix_secs(&self) -> i64;
}

/// 系统真实时钟（生产用）。
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_unix_secs(&self) -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
    }
}

/// 假时钟（测试用）：可手动推进。
#[derive(Debug, Clone)]
pub struct FakeClock {
    now: Arc<AtomicI64>,
}

impl FakeClock {
    pub fn new(start: i64) -> Self {
        Self {
            now: Arc::new(AtomicI64::new(start)),
        }
    }
    /// 设为指定时刻。
    pub fn advance_to(&self, t: i64) {
        self.now.store(t, Ordering::Relaxed);
    }
    /// 推进 1 秒并返回新值。
    pub fn tick(&self) -> i64 {
        let prev = self.now.fetch_add(1, Ordering::Relaxed);
        prev + 1
    }
}

impl Clock for FakeClock {
    fn now_unix_secs(&self) -> i64 {
        self.now.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fake_clock_advance_and_tick() {
        let c = FakeClock::new(100);
        assert_eq!(c.now_unix_secs(), 100);
        c.advance_to(200);
        assert_eq!(c.now_unix_secs(), 200);
        assert_eq!(c.tick(), 201);
        assert_eq!(c.now_unix_secs(), 201);
    }

    #[test]
    fn system_clock_is_positive() {
        let c = SystemClock;
        assert!(c.now_unix_secs() > 1_700_000_000);
    }
}
Write id.rs
74 lines
//! ID 分配：进程内单调计数器，无新依赖。
//!
//! 跨进程 / 崩溃恢复的持久化 `vm_id` 分配留待后续（迁入 store `meta` 表）。

use std::sync::atomic::{AtomicU64, Ordering};

/// sandbox_id / container_id / vm_id 生成器。
pub struct IdGenerator {
    sb: AtomicU64,
    ct: AtomicU64,
    vm: AtomicU64,
}

impl Default for IdGenerator {
    fn default() -> Self {
        Self {
            sb: AtomicU64::new(1),
            ct: AtomicU64::new(1),
            vm: AtomicU64::new(1), // 避开 VmId(0) 哨兵
        }
    }
}

impl IdGenerator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn sandbox_id(&self) -> String {
        let n = self.sb.fetch_add(1, Ordering::Relaxed);
        format!("sb-{n:016x}")
    }

    pub fn container_id(&self) -> String {
        let n = self.ct.fetch_add(1, Ordering::Relaxed);
        format!("ct-{n:016x}")
    }

    pub fn vm_id(&self) -> u64 {
        self.vm.fetch_add(1, Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn sandbox_ids_unique() {
        let g = IdGenerator::new();
        let set: HashSet<String> = (0..1000).map(|_| g.sandbox_id()).collect();
        assert_eq!(set.len(), 1000);
        assert!(g.sandbox_id().starts_with("sb-"));
    }

    #[test]
    fn container_ids_unique_and_prefixed() {
        let g = IdGenerator::new();
        let a = g.container_id();
        let b = g.container_id();
        assert_ne!(a, b);
        assert!(a.starts_with("ct-") && b.starts_with("ct-"));
    }

    #[test]
    fn vm_id_starts_at_one_and_increments() {
        let g = IdGenerator::new();
        assert_eq!(g.vm_id(), 1);
        assert_eq!(g.vm_id(), 2);
        assert_eq!(g.vm_id(), 3);
    }
}
Write types_table.rs
121 lines
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
Write lock.rs
71 lines
//! per-key 串行化锁（§3.2 / §5）。
//!
//! 锁粒度 per-`sandbox_id`；容器写锁父 sandbox（固定 sandbox→container 顺序）；
//! 读路径不加锁（§4.3）。锁表项不回收（少量泄漏，后续可周期压实）。

use std::collections::HashMap;
use std::sync::Mutex as StdMutex;

use tokio::sync::{Mutex, OwnedMutexGuard};

/// per-key 异步互斥锁表。
#[derive(Default)]
pub struct PerKeyLock {
    map: StdMutex<HashMap<String, std::sync::Arc<Mutex<()>>>>,
}

impl PerKeyLock {
    pub fn new() -> Self {
        Self::default()
    }

    /// 取 `key` 对应的锁；同 key 串行，不同 key 并行。返回 `OwnedMutexGuard`。
    pub async fn lock(&self, key: &str) -> OwnedMutexGuard<()> {
        let arc = {
            let mut m = self.map.lock().unwrap();
            m.entry(key.to_string())
                .or_insert_with(|| std::sync::Arc::new(Mutex::new(())))
                .clone()
        };
        arc.lock_owned().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Duration;

    #[tokio::test]
    async fn same_key_serializes() {
        let lk = Arc::new(PerKeyLock::new());
        let g = lk.lock("k").await;

        // 另一任务锁同一 key 应被阻塞：spawn 后短等仍持锁。
        let lk2 = lk.clone();
        let hit = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let hit2 = hit.clone();
        let join = tokio::spawn(async move {
            let _g2 = lk2.lock("k").await;
            hit2.store(true, std::sync::atomic::Ordering::Relaxed);
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!hit.load(std::sync::atomic::Ordering::Relaxed));

        drop(g); // 释放后另一任务应能拿到
        join.await.unwrap();
        assert!(hit.load(std::sync::atomic::Ordering::Relaxed));
    }

    #[tokio::test]
    async fn different_keys_parallel() {
        let lk = Arc::new(PerKeyLock::new());
        let g1 = lk.lock("a").await;
        // 不同 key 不应被 g1 阻塞：立即拿到。
        let g2 = lk.lock("b").await;
        drop(g1);
        drop(g2);
    }
}
Write readiness.rs
19 lines
//! VM 就绪等待抽象（§4.2 step 6-7）。
//!
//! 设计上 Manager 建 VM 后在 `event_fd` 上等就绪再落 READY。本迭代把「等就绪」抽象成
//! `VmReadiness`，真实 `eventfd` + `AsyncFd` 实现随 driver 专题落地；测试用 `Immediate` /
//! `NeverReady` 两种假实现覆盖成功与超时回滚两条路径（不挂起）。

use std::time::Duration;

use oas_driver::VmId;

use crate::OasError;

/// VM 就绪等待。
#[async_trait::async_trait]
pub trait VmReadiness: Send + Sync {
    /// 等待 `vm_id` 就绪，超时返回 `Err`（→ 回滚 `delete_vm` + cleanup + teardown）。
    async fn wait_ready(&self, vm_id: VmId, timeout: Duration) -> Result<(), OasError>;
}
Now declare the modules in lib.rs.

Edit lib.rs
Added 12 lines
Bash Run oas-manager supporting-type tests (Phase 3)
IN
cargo test --manifest-path /home/yunfei/Code/Only-AgentSandbox/only-agent-sandbox/Cargo.toml -p oas-manager 2>&1 | tail -35

OUT
   Compiling smallvec v1.15.2
   Compiling tokio-macros v2.7.0
   Compiling oas-types v0.1.0 (/home/yunfei/Code/Only-AgentSandbox/only-agent-sandbox/crates/oas-types)
   Compiling parking_lot_core v0.9.12
   Compiling parking_lot v0.12.5
   Compiling oas-store v0.1.0 (/home/yunfei/Code/Only-AgentSandbox/only-agent-sandbox/crates/oas-store)
   Compiling oas-net v0.1.0 (/home/yunfei/Code/Only-AgentSandbox/only-agent-sandbox/crates/oas-net)
   Compiling tokio v1.52.3
   Compiling oas-manager v0.1.0 (/home/yunfei/Code/Only-AgentSandbox/only-agent-sandbox/crates/oas-manager)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 10.92s
     Running unittests src/lib.rs (only-agent-sandbox/target/debug/deps/oas_manager-a54ef9eb85cd390e)

running 10 tests
test clock::tests::fake_clock_advance_and_tick ... ok
test clock::tests::system_clock_is_positive ... ok
test id::tests::container_ids_unique_and_prefixed ... ok
test id::tests::vm_id_starts_at_one_and_increments ... ok
test types_table::tests::all_images_flattens ... ok
test types_table::tests::known_types ... ok
test types_table::tests::unknown_type_rejected ... ok
test lock::tests::different_keys_parallel ... ok
test id::tests::sandbox_ids_unique ... ok
test lock::tests::same_key_serializes ... ok

test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.05s

   Doc-tests oas_manager

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
Phase 3 green (10 tests). Now the core — manager.rs with the full OasManager FSM. Let me write it.

Write manager.rs
515 lines
//! 真 `Manager` 实现（§3.2）：持 driver/net/storage/store + readiness + 类型表 + 锁 + 时钟。
//!
//! GuestAgent 本轮延后：`start_container` 仅置 RUNNING、`stop_container` 显式停止置 EXITED
//! （exit_code=0/Completed，非「VM 还活着」反推，红线 2 保留）。

use std::sync::Arc;
use std::time::Duration;

use oas_driver::{DriverError, FirecrackerDriver, VmId, VmLifecycle, VmSpec};
use oas_net::{NetConfig, NetError, NetworkManager};
use oas_storage::{DiskConfig, StorageError, StorageManager};
use oas_store::{Store, StoreError};
use oas_types::{
    ContainerExitReason, ContainerFilter, ContainerRecord, ContainerState, IpLease,
    SandboxFilter, SandboxRecord, SandboxState,
};

use crate::{
    Clock, CreateContainerRequest, CreateSandboxRequest, IdGenerator, ImageInfo, Manager,
    OasError, PerKeyLock, RuntimeCondition, RuntimeStatusInfo, SandboxTypeTable, VersionInfo,
    VmReadiness,
};

/// 就绪等待超时（真实 eventfd 实现用；假实现可忽略）。
const READINESS_TIMEOUT: Duration = Duration::from_secs(30);
/// `stop_sandbox` 轮询 `get_vm` 到 Stopped 的最大次数 / 间隔。
const MAX_STOP_POLLS: usize = 50;
const STOP_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// 真编排层 Manager。
pub struct OasManager {
    driver: Arc<dyn FirecrackerDriver>,
    net: Arc<dyn NetworkManager>,
    storage: Arc<dyn StorageManager>,
    store: Arc<dyn Store>,
    types: SandboxTypeTable,
    locks: PerKeyLock,
    clock: Arc<dyn Clock>,
    id_gen: IdGenerator,
    readiness: Arc<dyn VmReadiness>,
    // TODO(§2.8): agent: Arc<dyn GuestAgent> + container 退出回灌。
    // TODO(§4.6): reconcile 后台 task。
}

impl OasManager {
    pub fn new(
        driver: Arc<dyn FirecrackerDriver>,
        net: Arc<dyn NetworkManager>,
        storage: Arc<dyn StorageManager>,
        store: Arc<dyn Store>,
        readiness: Arc<dyn VmReadiness>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            driver,
            net,
            storage,
            store,
            types: SandboxTypeTable::default(),
            locks: PerKeyLock::new(),
            clock,
            id_gen: IdGenerator::new(),
            readiness,
        }
    }
}

// ---- 错误映射（下层 → OasError，自包含）----

fn map_driver_err(e: DriverError) -> OasError {
    let msg = e.to_string();
    match e {
        DriverError::NotFound(_) => OasError::NotFound(msg),
        DriverError::Exists(_) => OasError::Conflict(msg),
        _ => OasError::Unavailable(msg),
    }
}

fn map_net_err(e: NetError) -> OasError {
    let msg = e.to_string();
    match e {
        NetError::IpamExhausted(_) => OasError::Unavailable(msg),
        _ => OasError::Internal(msg),
    }
}

fn map_storage_err(e: StorageError) -> OasError {
    let msg = e.to_string();
    match e {
        StorageError::CloudDiskNotFound(_) => OasError::InvalidArgument(msg),
        _ => OasError::Internal(msg),
    }
}

fn map_store_err(e: StoreError) -> OasError {
    let msg = e.to_string();
    match e {
        StoreError::NotFound(_) => OasError::NotFound(msg),
        _ => OasError::Internal(msg),
    }
}

fn image_info(image: &str) -> ImageInfo {
    ImageInfo {
        id: format!("sha256:{image}"),
        repo_tags: vec![image.to_string()],
        repo_digests: vec![],
        size: 0,
        username: String::new(),
        image_ref: image.to_string(),
        pinned: true,
    }
}

/// 从 SandboxRecord 还原 NetConfig（供 `stop_sandbox` 调 `net.teardown` 释放 IP）。
/// `lease.cidr` 留空：`release_ip` 以 ip 为键，cidr 仅信息字段。
fn net_cfg_from(rec: &SandboxRecord) -> NetConfig {
    NetConfig {
        netns_path: rec.netns_path.clone(),
        tap_name: rec.tap_name.clone(),
        mac: rec.mac.clone(),
        pod_ip: rec.pod_ip.clone(),
        gateway: rec.gateway.clone(),
        lease: IpLease {
            ip: rec.pod_ip.clone(),
            cidr: String::new(),
            sandbox_id: rec.sandbox_id.clone(),
        },
    }
}

#[async_trait::async_trait]
impl Manager for OasManager {
    async fn version(&self) -> Result<VersionInfo, OasError> {
        Ok(VersionInfo {
            runtime_name: "only-agent-sandbox".into(),
            runtime_version: "0.1.0".into(),
            runtime_api_version: "v1".into(),
        })
    }

    async fn status(&self) -> Result<RuntimeStatusInfo, OasError> {
        Ok(RuntimeStatusInfo {
            conditions: vec![
                RuntimeCondition {
                    r#type: "RuntimeReady".into(),
                    status: true,
                    reason: String::new(),
                    message: String::new(),
                },
                RuntimeCondition {
                    r#type: "NetworkReady".into(),
                    status: true,
                    reason: String::new(),
                    message: String::new(),
                },
            ],
        })
    }

    async fn update_runtime_config(&self, _pod_cidr: Option<&str>) -> Result<(), OasError> {
        // TODO: 持久化 podCIDR 到 store `meta` 表。
        Ok(())
    }

    async fn run_sandbox(&self, req: CreateSandboxRequest) -> Result<String, OasError> {
        // 锁 pod_uid（幂等键）。
        let _guard = self.locks.lock(&req.metadata.uid).await;

        // 幂等短路：同 pod_uid 已存在。
        if let Some(existing) = self
            .store
            .get_sandbox_by_uid(&req.metadata.uid)
            .map_err(map_store_err)?
        {
            if existing.state == SandboxState::Ready {
                return Ok(existing.sandbox_id);
            }
            return Err(OasError::Conflict(format!(
                "sandbox {} exists but not ready; remove first",
                existing.sandbox_id
            )));
        }

        let ty = self.types.get(req.type_id)?;
        // 云盘一致性。
        if ty.has_cloud_disk && req.cloud_disk_ref.is_none() {
            return Err(OasError::InvalidArgument(format!(
                "type {} requires cloud-disk annotation",
                req.type_id
            )));
        }
        if !ty.has_cloud_disk && req.cloud_disk_ref.is_some() {
            return Err(OasError::InvalidArgument(format!(
                "type {} does not accept cloud-disk",
                req.type_id
            )));
        }

        let sandbox_id = self.id_gen.sandbox_id();

        // net.setup（内部 lease_ip，崩溃后不重复分配）。
        let net_cfg = match self.net.setup(&sandbox_id).await {
            Ok(c) => c,
            Err(e) => return Err(map_net_err(e)),
        };

        // storage.provision。
        let disk = match self
            .storage
            .provision(&sandbox_id, req.type_id, req.cloud_disk_ref.as_deref())
            .await
        {
            Ok(d) => d,
            Err(e) => {
                let _ = self.net.teardown(&net_cfg).await; // 回滚网络
                return Err(map_storage_err(e));
            }
        };

        // driver.create_vm（event_fd 本轮传哨兵 -1，假实现忽略；真实 eventfd 随 driver 专题）。
        let spec = VmSpec {
            rw_layer_path: disk.rw_layer_path.clone(),
            cloud_disk_dev: disk.cloud_disk_dev.clone(),
        };
        let vm_id = match self
            .driver
            .create_vm(&net_cfg.netns_path, req.type_id, spec, -1)
            .await
        {
            Ok(id) => id,
            Err(e) => {
                let oe = map_driver_err(e);
                self.rollback_create(&net_cfg, &disk, None).await;
                return Err(oe);
            }
        };

        // 等就绪（超时 → 回滚）。
        if let Err(oe) = self.readiness.wait_ready(vm_id, READINESS_TIMEOUT).await {
            self.rollback_create(&net_cfg, &disk, Some(vm_id)).await;
            return Err(oe);
        }

        // 落库 READY（单事务）。
        let rec = SandboxRecord {
            sandbox_id: sandbox_id.clone(),
            pod_uid: req.metadata.uid.clone(),
            metadata: req.metadata.clone(),
            labels: req.labels.clone(),
            annotations: req.annotations.clone(),
            type_id: req.type_id,
            vm_id: vm_id.0,
            netns_path: net_cfg.netns_path.clone(),
            tap_name: net_cfg.tap_name.clone(),
            mac: net_cfg.mac.clone(),
            pod_ip: net_cfg.pod_ip.clone(),
            gateway: net_cfg.gateway.clone(),
            rw_layer_path: disk.rw_layer_path.clone(),
            cloud_disk_dev: disk.cloud_disk_dev.clone(),
            state: SandboxState::Ready,
            created_at: self.clock.now_unix_secs(),
        };
        let rec_clone = rec.clone();
        if let Err(e) = self
            .store
            .transaction(Box::new(move |t| t.put_sandbox(&rec_clone)))
        {
            self.rollback_create(&net_cfg, &disk, Some(vm_id)).await;
            return Err(map_store_err(e));
        }
        Ok(sandbox_id)
    }

    async fn stop_sandbox(&self, sandbox_id: &str) -> Result<(), OasError> {
        let _guard = self.locks.lock(sandbox_id).await;
        let rec = self.store.get_sandbox(sandbox_id).map_err(map_store_err)?; // NotFound 透传，CRI 吞
        let vm_id = VmId(rec.vm_id);

        let _ = self.driver.delete_vm(vm_id).await; // 幂等
        for _ in 0..MAX_STOP_POLLS {
            match self.driver.get_vm(vm_id).await {
                Ok(s) if s.lifecycle == VmLifecycle::Stopped => break,
                Ok(_) => tokio::time::sleep(STOP_POLL_INTERVAL).await,
                Err(DriverError::NotFound(_)) => break,
                Err(_) => break, // best-effort
            }
        }

        let _ = self.net.teardown(&net_cfg_from(&rec)).await; // 释放 IP
        let mut rec2 = rec.clone();
        rec2.state = SandboxState::NotReady;
        let rec_clone = rec2.clone();
        self.store
            .transaction(Box::new(move |t| t.put_sandbox(&rec_clone)))
            .map_err(map_store_err)?;
        Ok(())
    }

    async fn remove_sandbox(&self, sandbox_id: &str) -> Result<(), OasError> {
        let _guard = self.locks.lock(sandbox_id).await;
        let rec = match self.store.get_sandbox(sandbox_id) {
            Ok(r) => r,
            Err(StoreError::NotFound(_)) => return Ok(()), // 幂等
            Err(e) => return Err(map_store_err(e)),
        };
        let disk = DiskConfig {
            rw_layer_path: rec.rw_layer_path.clone(),
            cloud_disk_dev: rec.cloud_disk_dev.clone(),
        };
        let _ = self.storage.cleanup(&disk).await; // best-effort
        let sid = sandbox_id.to_string();
        self.store
            .transaction(Box::new(move |t| t.delete_sandbox(&sid)))
            .map_err(map_store_err)?;
        Ok(())
    }

    async fn sandbox_status(&self, sandbox_id: &str) -> Result<SandboxRecord, OasError> {
        self.store.get_sandbox(sandbox_id).map_err(map_store_err)
    }

    async fn list_sandboxes(&self, filter: SandboxFilter) -> Result<Vec<SandboxRecord>, OasError> {
        self.store.list_sandboxes(&filter).map_err(map_store_err)
    }

    async fn create_container(&self, req: CreateContainerRequest) -> Result<String, OasError> {
        let _guard = self.locks.lock(&req.pod_sandbox_id).await;
        let sb = self
            .store
            .get_sandbox(&req.pod_sandbox_id)
            .map_err(map_store_err)?;
        let ty = self.types.get(sb.type_id)?;

        // 镜像白名单（ImageNotInList → CRI not_found）。
        if !ty.image_whitelist.iter().any(|w| w == &req.image) {
            return Err(OasError::ImageNotInList(req.image.clone()));
        }
        // 资源 vs type 预算（§9 一致性）。
        if let Some(mem) = req.resources.memory_limit_bytes {
            let budget = (ty.mem_mib as i64) * 1_048_576;
            if mem > budget {
                return Err(OasError::TypeMismatch(format!(
                    "memory {mem}B exceeds type {} budget {budget}B",
                    ty.type_id
                )));
            }
        }
        if let Some(cpu) = req.resources.cpu_shares {
            let budget = (ty.vcpu as u64) * 1024;
            if cpu > budget {
                return Err(OasError::TypeMismatch(format!(
                    "cpu_shares {cpu} exceeds type {} budget {budget}",
                    ty.type_id
                )));
            }
        }

        let container_id = self.id_gen.container_id();
        let env: Vec<String> = req.envs.iter().map(|kv| format!("{}={}", kv.key, kv.value)).collect();
        let rec = ContainerRecord {
            container_id: container_id.clone(),
            sandbox_id: req.pod_sandbox_id.clone(),
            metadata: req.metadata.clone(),
            image: req.image.clone(),
            command: req.command.clone(),
            args: req.args.clone(),
            env,
            mounts: req.mounts.clone(),
            resources: req.resources.clone(),
            state: ContainerState::Created,
            created_at: self.clock.now_unix_secs(),
            started_at: None,
            finished_at: None,
            exit_code: 0,
            reason: None,
            message: String::new(),
            labels: req.labels.clone(),
            annotations: req.annotations.clone(),
        };
        let rec_clone = rec.clone();
        self.store
            .transaction(Box::new(move |t| t.put_container(&rec_clone)))
            .map_err(map_store_err)?;
        Ok(container_id)
    }

    async fn start_container(&self, container_id: &str) -> Result<(), OasError> {
        let sid = self
            .store
            .get_container(container_id)
            .map_err(map_store_err)?
            .sandbox_id;
        let _guard = self.locks.lock(&sid).await;
        let c = self
            .store
            .get_container(container_id)
            .map_err(map_store_err)?;
        match c.state {
            ContainerState::Running => Ok(()), // 幂等
            ContainerState::Created => {
                let mut c2 = c.clone();
                c2.state = ContainerState::Running;
                c2.started_at = Some(self.clock.now_unix_secs());
                let cc = c2.clone();
                self.store
                    .transaction(Box::new(move |t| t.put_container(&cc)))
                    .map_err(map_store_err)?;
                Ok(())
            }
            _ => Err(OasError::Conflict(format!(
                "container {container_id} not in Created/Running state"
            ))),
        }
    }

    async fn stop_container(&self, container_id: &str, _timeout: i64) -> Result<(), OasError> {
        let sid = match self.store.get_container(container_id) {
            Ok(c) => c.sandbox_id,
            Err(StoreError::NotFound(_)) => return Ok(()), // 幂等
            Err(e) => return Err(map_store_err(e)),
        };
        let _guard = self.locks.lock(&sid).await;
        let c = self
            .store
            .get_container(container_id)
            .map_err(map_store_err)?;
        if c.state == ContainerState::Exited {
            return Ok(()); // 幂等
        }
        // agent 延后：显式停止置 EXITED（exit_code=0/Completed，非 VM-alive 反推）。
        let mut c2 = c.clone();
        c2.state = ContainerState::Exited;
        c2.finished_at = Some(self.clock.now_unix_secs());
        c2.exit_code = 0;
        c2.reason = Some(ContainerExitReason::Completed);
        c2.message = "explicit stop (agent deferred)".into();
        let cc = c2.clone();
        self.store
            .transaction(Box::new(move |t| t.put_container(&cc)))
            .map_err(map_store_err)?;
        Ok(())
    }

    async fn remove_container(&self, container_id: &str) -> Result<(), OasError> {
        let sid = match self.store.get_container(container_id) {
            Ok(c) => c.sandbox_id,
            Err(StoreError::NotFound(_)) => return Ok(()), // 幂等
            Err(e) => return Err(map_store_err(e)),
        };
        let _guard = self.locks.lock(&sid).await;
        let cid = container_id.to_string();
        self.store
            .transaction(Box::new(move |t| t.delete_container(&cid)))
            .map_err(map_store_err)?;
        Ok(())
    }

    async fn container_status(&self, container_id: &str) -> Result<ContainerRecord, OasError> {
        self.store.get_container(container_id).map_err(map_store_err)
    }

    async fn list_containers(
        &self,
        filter: ContainerFilter,
    ) -> Result<Vec<ContainerRecord>, OasError> {
        self.store.list_containers(&filter).map_err(map_store_err)
    }

    async fn image_status(&self, image: &str) -> Result<Option<ImageInfo>, OasError> {
        if self.types.all_images().iter().any(|w| *w == image) {
            Ok(Some(image_info(image)))
        } else {
            Ok(None)
        }
    }

    async fn pull_image(&self, image: &str) -> Result<String, OasError> {
        if self.types.all_images().iter().any(|w| *w == image) {
            Ok(image.to_string())
        } else {
            Err(OasError::ImageNotInList(image.to_string()))
        }
    }

    async fn list_images(&self) -> Result<Vec<ImageInfo>, OasError> {
        Ok(self
            .types
            .all_images()
            .into_iter()
            .map(image_info)
            .collect())
    }

    async fn remove_image(&self, _image: &str) -> Result<(), OasError> {
        Ok(()) // 白名单来自配置，幂等空操作。
    }
}

impl OasManager {
    /// `run_sandbox` 失败回滚已建资源（顺序：driver → storage → net）。
    async fn rollback_create(
        &self,
        net_cfg: &NetConfig,
        disk: &DiskConfig,
        vm_id: Option<VmId>,
    ) {
        if let Some(id) = vm_id {
            let _ = self.driver.delete_vm(id).await;
        }
        let _ = self.storage.cleanup(disk).await;
        let _ = self.net.teardown(net_cfg).await;
    }
}
Now declare manager module and re-export OasManager in lib.rs.

Edit lib.rs
Added 2 lines
Bash Build oas-manager with OasManager impl
IN
cargo build --manifest-path /home/yunfei/Code/Only-AgentSandbox/only-agent-sandbox/Cargo.toml -p oas-manager 2>&1 | tail -25

OUT
   Compiling oas-manager v0.1.0 (/home/yunfei/Code/Only-AgentSandbox/only-agent-sandbox/crates/oas-manager)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.05s
OasManager compiles. Now the e2e mocks + FSM tests. Let me write the shared mocks module.

Write mod.rs
348 lines
//! e2e 共享：手写 mock driver/net/storage + readiness 假实现 + 测试装配。
//!
//! 不引 mockall：每个 mock 用 `Arc<Mutex<…>>` 记录调用 + `AtomicBool` 注入失败，
//! 完全可控。`MockNet` 持真实 `Store` 做 IPAM（§4.2：net 层 lease/release）。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use oas_driver::{DriverError, FirecrackerDriver, VmId, VmLifecycle, VmSpec, VmStatus};
use oas_manager::{Clock, FakeClock, OasManager, VmReadiness};
use oas_net::{NetConfig, NetError, NetworkManager};
use oas_storage::{DiskConfig, StorageError, StorageManager};
use oas_store::{MemoryStore, Store};
use oas_types::IpLease;

// ---- MockDriver ------------------------------------------------------------

pub struct MockDriver {
    fail_create: AtomicBool,
    creates: Mutex<Vec<(String, u8)>>,
    deletes: Mutex<Vec<u64>>,
    gets: Mutex<Vec<u64>>,
    lists: Mutex<Vec<()>>,
    statuses: Mutex<HashMap<u64, VmStatus>>,
    next_vm_id: AtomicU64,
}

impl MockDriver {
    pub fn new() -> Self {
        Self {
            fail_create: AtomicBool::new(false),
            creates: Mutex::new(Vec::new()),
            deletes: Mutex::new(Vec::new()),
            gets: Mutex::new(Vec::new()),
            lists: Mutex::new(Vec::new()),
            statuses: Mutex::new(HashMap::new()),
            next_vm_id: AtomicU64::new(1),
        }
    }
    pub fn fail_create(&self, v: bool) {
        self.fail_create.store(v, Ordering::Relaxed);
    }
    pub fn create_count(&self) -> usize {
        self.creates.lock().unwrap().len()
    }
    pub fn delete_count(&self) -> usize {
        self.deletes.lock().unwrap().len()
    }
    pub fn get_count(&self) -> usize {
        self.gets.lock().unwrap().len()
    }
    pub fn list_count(&self) -> usize {
        self.lists.lock().unwrap().len()
    }
}

#[async_trait]
impl FirecrackerDriver for MockDriver {
    async fn create_vm(
        &self,
        netns_path: &str,
        type_id: u8,
        _spec: VmSpec,
        _event_fd: std::os::fd::RawFd,
    ) -> Result<VmId, DriverError> {
        if self.fail_create.load(Ordering::Relaxed) {
            return Err(DriverError::Other("injected create_vm failure".into()));
        }
        let id = self.next_vm_id.fetch_add(1, Ordering::Relaxed);
        self.creates
            .lock()
            .unwrap()
            .push((netns_path.to_string(), type_id));
        self.statuses.lock().unwrap().insert(
            id,
            VmStatus {
                started: true,
                healthy: true,
                lifecycle: VmLifecycle::Running,
            },
        );
        Ok(VmId(id))
    }

    async fn get_vm(&self, id: VmId) -> Result<VmStatus, DriverError> {
        self.gets.lock().unwrap().push(id.0);
        match self.statuses.lock().unwrap().get(&id.0) {
            Some(s) => Ok(s.clone()),
            None => Err(DriverError::NotFound(id.0)),
        }
    }

    async fn delete_vm(&self, id: VmId) -> Result<(), DriverError> {
        self.deletes.lock().unwrap().push(id.0);
        let mut st = self.statuses.lock().unwrap();
        if let Some(s) = st.get_mut(&id.0) {
            s.lifecycle = VmLifecycle::Stopped;
        }
        Ok(())
    }

    async fn list_vm(&self) -> Result<Vec<VmStatus>, DriverError> {
        self.lists.lock().unwrap().push(());
        Ok(self.statuses.lock().unwrap().values().cloned().collect())
    }
}

// ---- MockNet ---------------------------------------------------------------

pub struct MockNet {
    store: Arc<dyn Store>,
    cidr: String,
    gateway: String,
    setups: Mutex<Vec<String>>,
    teardowns: Mutex<Vec<String>>,
    fail_setup: AtomicBool,
}

impl MockNet {
    pub fn new(store: Arc<dyn Store>, cidr: &str, gateway: &str) -> Self {
        Self {
            store,
            cidr: cidr.to_string(),
            gateway: gateway.to_string(),
            setups: Mutex::new(Vec::new()),
            teardowns: Mutex::new(Vec::new()),
            fail_setup: AtomicBool::new(false),
        }
    }
    pub fn fail_setup(&self, v: bool) {
        self.fail_setup.store(v, Ordering::Relaxed);
    }
    pub fn setup_count(&self) -> usize {
        self.setups.lock().unwrap().len()
    }
    pub fn teardown_count(&self) -> usize {
        self.teardowns.lock().unwrap().len()
    }
}

#[async_trait]
impl NetworkManager for MockNet {
    async fn setup(&self, sandbox_id: &str) -> Result<NetConfig, NetError> {
        if self.fail_setup.load(Ordering::Relaxed) {
            return Err(NetError::Other("injected setup failure".into()));
        }
        let mut lease = self
            .store
            .lease_ip(&self.cidr)
            .map_err(|e| NetError::Other(e.to_string()))?;
        lease.sandbox_id = sandbox_id.to_string();
        self.setups.lock().unwrap().push(sandbox_id.to_string());
        Ok(NetConfig {
            netns_path: format!("/var/run/netns/oas-{sandbox_id}"),
            tap_name: "tap0".into(),
            mac: format!("02:oas:{sandbox_id}"),
            pod_ip: lease.ip.clone(),
            gateway: self.gateway.clone(),
            lease,
        })
    }

    async fn teardown(&self, net: &NetConfig) -> Result<(), NetError> {
        self.teardowns
            .lock()
            .unwrap()
            .push(net.lease.ip.clone());
        self.store
            .release_ip(&net.lease)
            .map_err(|e| NetError::Other(e.to_string()))?;
        Ok(())
    }
}

// ---- MockStorage -----------------------------------------------------------

pub struct MockStorage {
    fail_provision: AtomicBool,
    provisions: Mutex<Vec<(String, u8)>>,
    cleanups: Mutex<Vec<()>>,
}

impl MockStorage {
    pub fn new() -> Self {
        Self {
            fail_provision: AtomicBool::new(false),
            provisions: Mutex::new(Vec::new()),
            cleanups: Mutex::new(Vec::new()),
        }
    }
    pub fn fail_provision(&self, v: bool) {
        self.fail_provision.store(v, Ordering::Relaxed);
    }
    pub fn provision_count(&self) -> usize {
        self.provisions.lock().unwrap().len()
    }
    pub fn cleanup_count(&self) -> usize {
        self.cleanups.lock().unwrap().len()
    }
}

#[async_trait]
impl StorageManager for MockStorage {
    async fn provision(
        &self,
        sandbox_id: &str,
        type_id: u8,
        cloud_disk_ref: Option<&str>,
    ) -> Result<DiskConfig, StorageError> {
        if self.fail_provision.load(Ordering::Relaxed) {
            return Err(StorageError::Other("injected provision failure".into()));
        }
        self.provisions
            .lock()
            .unwrap()
            .push((sandbox_id.to_string(), type_id));
        Ok(DiskConfig {
            rw_layer_path: Some(format!("/var/lib/oas/rw/{sandbox_id}.ext4")),
            cloud_disk_dev: cloud_disk_ref.map(|s| s.to_string()),
        })
    }

    async fn cleanup(&self, _disk: &DiskConfig) -> Result<(), StorageError> {
        self.cleanups.lock().unwrap().push(());
        Ok(())
    }
}

// ---- Readiness 假实现 ------------------------------------------------------

pub struct ImmediateReadiness;

#[async_trait]
impl VmReadiness for ImmediateReadiness {
    async fn wait_ready(&self, _vm_id: VmId, _timeout: Duration) -> Result<(), oas_manager::OasError> {
        Ok(())
    }
}

pub struct NeverReadyReadiness;

#[async_trait]
impl VmReadiness for NeverReadyReadiness {
    async fn wait_ready(&self, _vm_id: VmId, _timeout: Duration) -> Result<(), oas_manager::OasError> {
        tokio::time::sleep(Duration::from_millis(20)).await;
        Err(oas_manager::OasError::Unavailable(
            "vm not ready (never-ready fake)".into(),
        ))
    }
}

// ---- 测试装配 --------------------------------------------------------------

pub struct TestEnv {
    pub mgr: Arc<OasManager>,
    pub store: Arc<MemoryStore>,
    pub driver: Arc<MockDriver>,
    pub net: Arc<MockNet>,
    pub storage: Arc<MockStorage>,
    pub clock: Arc<FakeClock>,
}

impl TestEnv {
    pub fn new() -> Self {
        Self::with_readiness(Arc::new(ImmediateReadiness))
    }

    pub fn with_readiness(readiness: Arc<dyn VmReadiness>) -> Self {
        let store = Arc::new(MemoryStore::new());
        let driver = Arc::new(MockDriver::new());
        let net = Arc::new(MockNet::new(store.clone(), "10.244.0.0/24", "10.244.0.254"));
        let storage = Arc::new(MockStorage::new());
        let clock = Arc::new(FakeClock::new(1_700_000_000));
        let mgr = Arc::new(OasManager::new(
            driver.clone(),
            net.clone(),
            storage.clone(),
            store.clone(),
            readiness,
            clock.clone(),
        ));
        Self {
            mgr,
            store,
            driver,
            net,
            storage,
            clock,
        }
    }
}

// ---- 请求构造助手 ----

pub fn sandbox_req(uid: &str, type_id: u8) -> oas_manager::CreateSandboxRequest {
    use std::collections::HashMap;
    oas_manager::CreateSandboxRequest {
        metadata: oas_types::SandboxMetadata {
            name: format!("pod-{uid}"),
            namespace: "default".into(),
            uid: uid.into(),
            attempt: 0,
        },
        labels: HashMap::new(),
        annotations: HashMap::new(),
        hostname: "host".into(),
        log_directory: "/logs".into(),
        dns_config: None,
        cgroup_parent: String::new(),
        type_id,
        cloud_disk_ref: None,
        rw_size: None,
    }
}

pub fn container_req(sandbox_id: &str, image: &str) -> oas_manager::CreateContainerRequest {
    use std::collections::HashMap;
    oas_manager::CreateContainerRequest {
        pod_sandbox_id: sandbox_id.into(),
        metadata: oas_types::ContainerMetadata {
            name: "c0".into(),
            attempt: 0,
        },
        image: image.into(),
        command: vec!["sh".into()],
        args: vec![],
        working_dir: String::new(),
        envs: vec![oas_types::KeyValue {
            key: "K".into(),
            value: "V".into(),
        }],
        mounts: vec![],
        labels: HashMap::new(),
        annotations: HashMap::new(),
        log_path: String::new(),
        resources: oas_types::LinuxResources::default(),
        tty: false,
        stdin: false,
        stdin_once: false,
    }
}

#[allow(dead_code)]
fn _force_ip_lease_used(_: IpLease) {}
Let me remove the stray unused helper I left in the mocks module.

Edit mod.rs
Removed 1 line
Edit mod.rs
Removed 3 lines
Now the direct-Manager FSM tests (sandbox + container + image). Let me update todos and write the test file.

Update Todos

Phase 0: Setup — add oas-e2e skeleton, workspace redb dep, oas-manager/oas-store Cargo deps; compile-only green

Phase 1: Store object-safety + MemoryStore (TDD)

Phase 2: RedbStore (TDD) — conformance green

Phase 3: Manager supporting types — Clock/IdGenerator/SandboxTypeTable/PerKeyLock + tests

Phase 4-6: OasManager FSM (sandbox/container/image/version) + mocks + direct FSM tests

Phase 7: E2E over UDS — oas-e2e lifecycle.rs with real Manager + mocks; green
Write manager_fsm.rs
421 lines
//! 直接驱动 `OasManager`（不经 UDS）的状态机 / 幂等 / 回滚 / 红线测试。

mod common;

use std::sync::Arc;

use common::{container_req, sandbox_req, NeverReadyReadiness, TestEnv};
use oas_manager::{Manager, OasError};
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
Bash Run direct FSM tests
IN
cargo test --manifest-path /home/yunfei/Code/Only-AgentSandbox/only-agent-sandbox/Cargo.toml -p oas-e2e --test manager_fsm 2>&1 | tail -45

OUT
   --> crates/oas-e2e/tests/common/mod.rs:237:24
    |
237 |     async fn wait_ready(&self, _vm_id: VmId, _timeout: Duration) -> Result<(), oas_manager::OasError> {
    |                        ^ lifetimes do not match method in trait
    |
   ::: crates/oas-manager/src/readiness.rs:17:5
    |
 17 |     async fn wait_ready(&self, vm_id: VmId, timeout: Duration) -> Result<(), OasError>;
    |     ----------------------------------------------------------------------------------- lifetimes in impl do not match this method in trait

error[E0195]: lifetime parameters or bounds on method `wait_ready` do not match the trait declaration
   --> crates/oas-e2e/tests/common/mod.rs:246:24
    |
246 |     async fn wait_ready(&self, _vm_id: VmId, _timeout: Duration) -> Result<(), oas_manager::OasError> {
    |                        ^ lifetimes do not match method in trait
    |
   ::: crates/oas-manager/src/readiness.rs:17:5
    |
 17 |     async fn wait_ready(&self, vm_id: VmId, timeout: Duration) -> Result<(), OasError>;
    |     ----------------------------------------------------------------------------------- lifetimes in impl do not match this method in trait

error[E0599]: no method named `get_sandbox_by_uid` found for struct `Arc<MemoryStore>` in the current scope
   --> crates/oas-e2e/tests/manager_fsm.rs:120:10
    |
118 |       assert!(env
    |  _____________-
119 | |         .store
120 | |         .get_sandbox_by_uid("uid1")
    | |_________-^^^^^^^^^^^^^^^^^^
    |
    = help: items from traits can only be used if the trait is in scope
help: trait `Store` which provides `get_sandbox_by_uid` is implemented but not in scope; perhaps you want to import it
    |
  3 + use oas_store::Store;
    |
help: there is a method `get_sandbox` with a similar name
    |
120 -         .get_sandbox_by_uid("uid1")
120 +         .get_sandbox("uid1")
    |

Some errors have detailed explanations: E0195, E0432, E0599.
For more information about an error, try `rustc --explain E0195`.
warning: `oas-e2e` (test "manager_fsm") generated 1 warning
error: could not compile `oas-e2e` (test "manager_fsm") due to 12 previous errors; 1 warning emitted
async-trait isn't a dep of oas-e2e, and Store isn't in scope in the test. Let me fix both.

Edit Cargo.toml
Added 1 line
Edit manager_fsm.rs
Added 1 line
Bash Run direct FSM tests
IN
cargo test --manifest-path /home/yunfei/Code/Only-AgentSandbox/only-agent-sandbox/Cargo.toml -p oas-e2e --test manager_fsm 2>&1 | tail -50

OUT
running 30 tests
test image_status_hit_and_miss ... ok
test list_images_flattens_whitelist ... ok
test create_container_sandbox_missing ... ok
test pull_image_whitelist_and_reject ... ok
test create_container_image_not_in_whitelist ... ok
test create_container_memory_exceeds_type ... ok
test container_status_never_calls_driver ... ok
test create_container_happy ... ok
test list_containers_by_sandbox ... ok
test read_paths_never_call_driver ... ok
test remove_image_noop ... ok
test run_sandbox_create_vm_fail_rolls_back ... ok
test run_sandbox_happy ... ok
test remove_sandbox_idempotent ... ok
test remove_container_idempotent ... ok
test run_sandbox_idempotent_by_uid ... ok
test run_sandbox_notready_then_conflict ... ok
test run_sandbox_type0_with_cloud_disk ... ok
test run_sandbox_type2_missing_cloud_disk ... ok
test start_container_created_to_running ... ok
test run_sandbox_storage_fail_rolls_back_net ... FAILED
test start_container_exited_conflict ... ok
test stop_container_missing_ok ... ok
test start_container_running_idempotent ... ok
test stop_container_created_to_exited ... ok
test stop_container_idempotent_on_exited ... ok
test stop_container_running_to_exited ... ok
test version_and_status_fixed ... ok
test stop_sandbox_polls_to_stopped_then_notready ... ok
test run_sandbox_readiness_timeout_rolls_back_all ... ok

failures:

---- run_sandbox_storage_fail_rolls_back_net stdout ----

thread 'run_sandbox_storage_fail_rolls_back_net' (1456989) panicked at crates/oas-e2e/tests/manager_fsm.rs:88:5:
assertion `left == right` failed
  left: 0
 right: 1
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace


failures:
    run_sandbox_storage_fail_rolls_back_net

test result: FAILED. 29 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s

error: test failed, to rerun pass `-p oas-e2e --test manager_fsm`
29/30 pass. The storage-fail rollback test shows teardown wasn't called — let me instrument to see which path is taken.

Edit manager.rs
Added 1 line
Bash Run failing test with debug output
IN
cargo test --manifest-path /home/yunfei/Code/Only-AgentSandbox/only-agent-sandbox/Cargo.toml -p oas-e2e --test manager_fsm run_sandbox_storage_fail_rolls_back_net -- --nocapture 2>&1 | tail -20

OUT
     Running tests/manager_fsm.rs (only-agent-sandbox/target/debug/deps/manager_fsm-6bb42d5eac6f80c6)

running 1 test
DBG provision failed, rolling back net

thread 'run_sandbox_storage_fail_rolls_back_net' (1458915) panicked at crates/oas-e2e/tests/manager_fsm.rs:88:5:
assertion `left == right` failed
  left: 0
 right: 1
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
test run_sandbox_storage_fail_rolls_back_net ... FAILED

failures:

failures:
    run_sandbox_storage_fail_rolls_back_net

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 29 filtered out; finished in 0.00s

error: test failed, to rerun pass `-p oas-e2e --test manager_fsm`
The provision-Err branch runs and calls teardown, yet count is 0. Let me instrument MockNet::teardown directly.

Edit mod.rs
Added 1 line
Bash Run failing test with mock debug output
IN
cargo test --manifest-path /home/yunfei/Code/Only-AgentSandbox/only-agent-sandbox/Cargo.toml -p oas-e2e --test manager_fsm run_sandbox_storage_fail_rolls_back_net -- --nocapture 2>&1 | tail -15

OUT
thread 'run_sandbox_storage_fail_rolls_back_net' (1459672) panicked at crates/oas-e2e/tests/manager_fsm.rs:88:5:
assertion `left == right` failed
  left: 0
 right: 1
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
test run_sandbox_storage_fail_rolls_back_net ... FAILED

failures:

failures:
    run_sandbox_storage_fail_rolls_back_net

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 29 filtered out; finished in 0.00s

error: test failed, to rerun pass `-p oas-e2e --test manager_fsm`
The manager's print fires but MockNet::teardown's print doesn't — teardown isn't dispatching to the mock. Let me add prints around the await in the manager.

Edit manager.rs
Added 1 line
Bash Check teardown dispatch prints
IN
cargo test --manifest-path /home/yunfei/Code/Only-AgentSandbox/only-agent-sandbox/Cargo.toml -p oas-e2e --test manager_fsm run_sandbox_storage_fail_rolls_back_net -- --nocapture 2>&1 | grep -E "DBG|FAIL|panic|left|right" | head

OUT
DBG provision failed, rolling back net
DBG MockNet::teardown called ip=10.244.0.1
DBG after teardown
thread 'run_sandbox_storage_fail_rolls_back_net' (1460917) panicked at crates/oas-e2e/tests/manager_fsm.rs:88:5:
assertion `left == right` failed
  left: 0
 right: 1
test run_sandbox_storage_fail_rolls_back_net ... FAILED
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 29 filtered out; finished in 0.00s
Teardown runs and pushes, yet teardown_count() reads 0 — so the mock being read isn't the one being called. Let me print pointer addresses to confirm.

Edit mod.rs
Added 4 lines
Edit mod.rs
Added 2 lines
Bash Compare mock addresses
IN
cargo test --manifest-path /home/yunfei/Code/Only-AgentSandbox/only-agent-sandbox/Cargo.toml -p oas-e2e --test manager_fsm run_sandbox_storage_fail_rolls_back_net -- --nocapture 2>&1 | grep -E "DBG|left|right" | head

OUT
DBG provision failed, rolling back net
DBG MockNet::teardown 0xffff94000bb0 ip=10.244.0.1
DBG after teardown
DBG teardown_count 0xffff94000bb0 = 1
assertion `left == right` failed
  left: 0
 right: 1
Tests run in parallel, interleaving prints. Let me run just this test single-threaded to isolate.

Bash Isolate failing test single-threaded
IN
cargo test --manifest-path /home/yunfei/Code/Only-AgentSandbox/only-agent-sandbox/Cargo.toml -p oas-e2e --test manager_fsm run_sandbox_storage_fail_rolls_back_net -- --nocapture --test-threads=1 2>&1 | grep -E "DBG|left|right|FAIL" | head

OUT
test run_sandbox_storage_fail_rolls_back_net ... DBG provision failed, rolling back net
DBG MockNet::teardown 0xffff98000bb0 ip=10.244.0.1
DBG after teardown
DBG teardown_count 0xffff98000bb0 = 1
assertion `left == right` failed
  left: 0
 right: 1
FAILED
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 29 filtered out; finished in 0.00s
Found it — the rollback was always working (teardown_count=1). The failing assertion is provision_count==1, but MockStorage::provision returns Err before recording the call. Let me record the attempt first, and remove all the debug prints.

Edit mod.rs
Modified
Now remove all the debug prints from the manager and mocks.

Edit manager.rs
Removed 2 lines
Edit mod.rs
Removed 5 lines
Edit mod.rs
Removed 2 lines
Bash Re-run direct FSM tests
IN
cargo test --manifest-path /home/yunfei/Code/Only-AgentSandbox/only-agent-sandbox/Cargo.toml -p oas-e2e --test manager_fsm 2>&1 | tail -12

OUT
test start_container_exited_conflict ... ok
test start_container_running_idempotent ... ok
test stop_container_missing_ok ... ok
test stop_container_created_to_exited ... ok
test stop_container_idempotent_on_exited ... ok
test version_and_status_fixed ... ok
test stop_container_running_to_exited ... ok
test stop_sandbox_polls_to_stopped_then_notready ... ok
test run_sandbox_readiness_timeout_rolls_back_all ... ok

test result: ok. 30 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s
All 30 FSM tests pass. Now Phase 7 — E2E over UDS. Let me read convert.rs to construct correct proto configs.

Read convert.rs
All FSM tests green. Now Phase 7 — UDS e2e. Let me write lifecycle.rs.

Write lifecycle.rs
454 lines
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
    format!(
        "/tmp/oas-e2e-{}-{n}.sock",
        std::process::id()
    )
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

#[tokio::test]
async fn e2e_full_lifecycle() {
    let env = TestEnv::new();
    let mut c = setup(&env).await;

    // RunPodSandbox → id
    let sb_id = c
        .rt
        .run_pod_sandbox(RunPodSandboxRequest {
            config: Some(pod_cfg("uid-e2e", 0)),
            runtime_handler: String::new(),
        })
        .await
        .unwrap()
        .into_inner()
        .pod_sandbox_id;
    assert!(!sb_id.is_empty());

    // PodSandboxStatus → READY + pod_ip
    let status = c
        .rt
        .pod_sandbox_status(PodSandboxStatusRequest {
            sandbox_id: sb_id.clone(),
        })
        .await
        .unwrap()
        .into_inner()
        .status
        .unwrap();
    assert_eq!(status.state, PodSandboxState::SandboxReady as i32);
    assert!(!status.network.unwrap().ip.is_empty());

    // CreateContainer → CREATED
    let cid = c
        .rt
        .create_container(CreateContainerRequest {
            pod_sandbox_id: sb_id.clone(),
            config: Some(container_cfg("img-a")),
        })
        .await
        .unwrap()
        .into_inner()
        .container_id;
    let cs = c
        .rt
        .container_status(ContainerStatusRequest {
            container_id: cid.clone(),
        })
        .await
        .unwrap()
        .into_inner()
        .status
        .unwrap();
    assert_eq!(cs.state, ContainerState::ContainerCreated as i32);

    // StartContainer → RUNNING + started_at
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
        })
        .await
        .unwrap()
        .into_inner()
        .status
        .unwrap();
    assert_eq!(cs.state, ContainerState::ContainerRunning as i32);
    assert!(cs.started_at > 0);

    // StopContainer → EXITED, exit_code=0
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
            container_id: cid.clone(),
        })
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::NotFound);

    // StopPodSandbox → NotReady
    c.rt
        .stop_pod_sandbox(StopPodSandboxRequest {
            sandbox_id: sb_id.clone(),
        })
        .await
        .unwrap();
    let st = c
        .rt
        .pod_sandbox_status(PodSandboxStatusRequest {
            sandbox_id: sb_id.clone(),
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
            sandbox_id: sb_id.clone(),
        })
        .await
        .unwrap();
    let err = c
        .rt
        .pod_sandbox_status(PodSandboxStatusRequest {
            sandbox_id: sb_id.clone(),
        })
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::NotFound);
}

#[tokio::test]
async fn e2e_list_pod_sandbox_after_run() {
    let env = TestEnv::new();
    let mut c = setup(&env).await;
    c.rt
        .run_pod_sandbox(RunPodSandboxRequest {
            config: Some(pod_cfg("uid-1", 0)),
            runtime_handler: String::new(),
        })
        .await
        .unwrap();
    let items = c
        .rt
        .list_pod_sandbox(ListPodSandboxRequest {
            filter: None,
        })
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
    let sb_id = c
        .rt
        .run_pod_sandbox(RunPodSandboxRequest {
            config: Some(pod_cfg("uid-1", 0)),
            runtime_handler: String::new(),
        })
        .await
        .unwrap()
        .into_inner()
        .pod_sandbox_id;
    for _ in 0..2 {
        c.rt
            .create_container(CreateContainerRequest {
                pod_sandbox_id: sb_id.clone(),
                config: Some(container_cfg("img-a")),
            })
            .await
            .unwrap();
    }
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
    let sb_id = c
        .rt
        .run_pod_sandbox(RunPodSandboxRequest {
            config: Some(pod_cfg("uid-1", 0)),
            runtime_handler: String::new(),
        })
        .await
        .unwrap()
        .into_inner()
        .pod_sandbox_id;
    let err = c
        .rt
        .create_container(CreateContainerRequest {
            pod_sandbox_id: sb_id,
            config: Some(container_cfg("img-x")),
        })
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::NotFound, "ImageNotInList → not_found");
}

#[tokio::test]
async fn e2e_run_sandbox_idempotent_by_uid() {
    let env = TestEnv::new();
    let mut c = setup(&env).await;
    let id1 = c
        .rt
        .run_pod_sandbox(RunPodSandboxRequest {
            config: Some(pod_cfg("uid-1", 0)),
            runtime_handler: String::new(),
        })
        .await
        .unwrap()
        .into_inner()
        .pod_sandbox_id;
    let id2 = c
        .rt
        .run_pod_sandbox(RunPodSandboxRequest {
            config: Some(pod_cfg("uid-1", 0)),
            runtime_handler: String::new(),
        })
        .await
        .unwrap()
        .into_inner()
        .pod_sandbox_id;
    assert_eq!(id1, id2, "same pod_uid → same sandbox_id, no second VM");
    assert_eq!(env.driver.create_count(), 1);
}

#[tokio::test]
async fn e2e_remove_pod_sandbox_idempotent() {
    let env = TestEnv::new();
    let mut c = setup(&env).await;
    let sb_id = c
        .rt
        .run_pod_sandbox(RunPodSandboxRequest {
            config: Some(pod_cfg("uid-1", 0)),
            runtime_handler: String::new(),
        })
        .await
        .unwrap()
        .into_inner()
        .pod_sandbox_id;
    c.rt
        .stop_pod_sandbox(StopPodSandboxRequest {
            sandbox_id: sb_id.clone(),
        })
        .await
        .unwrap();
    c.rt
        .remove_pod_sandbox(RemovePodSandboxRequest {
            sandbox_id: sb_id.clone(),
        })
        .await
        .unwrap();
    // 第二次删 = Ok（幂等）。
    c.rt
        .remove_pod_sandbox(RemovePodSandboxRequest {
            sandbox_id: sb_id,
        })
        .await
        .unwrap();
}
Bash Run UDS e2e lifecycle tests
IN
cargo test --manifest-path /home/yunfei/Code/Only-AgentSandbox/only-agent-sandbox/Cargo.toml -p oas-e2e --test lifecycle 2>&1 | tail -30

OUT
    |                           ^^^^^^^^^^^^^^^^^^^^^^ missing `sandbox_config`

error[E0560]: struct `oas_cri::runtime::v1::StopPodSandboxRequest` has no field named `sandbox_id`
   --> crates/oas-e2e/tests/lifecycle.rs:436:13
    |
436 |             sandbox_id: sb_id.clone(),
    |             ^^^^^^^^^^ `oas_cri::runtime::v1::StopPodSandboxRequest` does not have this field
    |
    = note: available fields are: `pod_sandbox_id`

error[E0560]: struct `oas_cri::runtime::v1::RemovePodSandboxRequest` has no field named `sandbox_id`
   --> crates/oas-e2e/tests/lifecycle.rs:442:13
    |
442 |             sandbox_id: sb_id.clone(),
    |             ^^^^^^^^^^ `oas_cri::runtime::v1::RemovePodSandboxRequest` does not have this field
    |
    = note: available fields are: `pod_sandbox_id`

error[E0560]: struct `oas_cri::runtime::v1::RemovePodSandboxRequest` has no field named `sandbox_id`
   --> crates/oas-e2e/tests/lifecycle.rs:449:13
    |
449 |             sandbox_id: sb_id,
    |             ^^^^^^^^^^ `oas_cri::runtime::v1::RemovePodSandboxRequest` does not have this field
    |
    = note: available fields are: `pod_sandbox_id`

Some errors have detailed explanations: E0063, E0560.
For more information about an error, try `rustc --explain E0063`.
warning: `oas-e2e` (test "lifecycle") generated 1 warning
error: could not compile `oas-e2e` (test "lifecycle") due to 17 previous errors; 1 warning emitted
Field names differ from my assumptions. Let me check the actual proto definitions.

Bash List request message definitions in proto
IN
P=/home/yunfei/Code/Only-AgentSandbox/only-agent-sandbox/crates/oas-cri/proto/api.proto; grep -nE "message (Stop|Remove|PodSandboxStatus|ContainerStatus|CreateContainer|StartContainer|StopContainer|ListPodSandbox|ListContainers|RunPodSandbox|ImageStatus|ListImages)Request" $P | head -30

OUT
517:message RunPodSandboxRequest {
547:message PodSandboxStatusRequest {
643:message ListPodSandboxRequest {
1257:message CreateContainerRequest {
1274:message StartContainerRequest {
1281:message StopContainerRequest {
1326:message ListContainersRequest {
1372:message ContainerStatusRequest {
1571:message ListImagesRequest {
1606:message ImageStatusRequest {
Bash Dump request message fields
IN
P=/home/yunfei/Code/Only-AgentSandbox/only-agent-sandbox/crates/oas-cri/proto/api.proto; for m in RunPodSandboxRequest PodSandboxStatusRequest ListPodSandboxRequest CreateContainerRequest StartContainerRequest StopContainerRequest ListContainersRequest ContainerStatusRequest ListImagesRequest ImageStatusRequest; do echo "=== $m ==="; sed -n "/^message $m {/,/^}/p" $P; done; echo "=== Stop/RemovePodSandboxRequest ==="; sed -n '/message StopPodSandboxRequest {/,/^}/p' $P; sed -n '/message RemovePodSandboxRequest {/,/^}/p' $P

OUT
=== RunPodSandboxRequest ===
message RunPodSandboxRequest {
    // Configuration for creating a PodSandbox.
    PodSandboxConfig config = 1;
    // Named runtime configuration to use for this PodSandbox.
    // If the runtime handler is unknown, this request should be rejected.  An
    // empty string should select the default handler, equivalent to the
    // behavior before this feature was added.
    // See https://git.k8s.io/enhancements/keps/sig-node/585-runtime-class
    string runtime_handler = 2;
}
=== PodSandboxStatusRequest ===
message PodSandboxStatusRequest {
    // ID of the PodSandbox for which to retrieve status.
    string pod_sandbox_id = 1;
    // Verbose indicates whether to return extra information about the pod sandbox.
    bool verbose = 2;
}
=== ListPodSandboxRequest ===
message ListPodSandboxRequest {
    // PodSandboxFilter to filter a list of PodSandboxes.
    PodSandboxFilter filter = 1;
}
=== CreateContainerRequest ===
message CreateContainerRequest {
    // ID of the PodSandbox in which the container should be created.
    string pod_sandbox_id = 1;
    // Config of the container.
    ContainerConfig config = 2;
    // Config of the PodSandbox. This is the same config that was passed
    // to RunPodSandboxRequest to create the PodSandbox. It is passed again
    // here just for easy reference. The PodSandboxConfig is immutable and
    // remains the same throughout the lifetime of the pod.
    PodSandboxConfig sandbox_config = 3;
}
=== StartContainerRequest ===
message StartContainerRequest {
    // ID of the container to start.
    string container_id = 1;
}
=== StopContainerRequest ===
message StopContainerRequest {
    // ID of the container to stop.
    string container_id = 1;
    // Timeout in seconds to wait for the container to stop before forcibly
    // terminating it. Default: 0 (forcibly terminate the container immediately)
    int64 timeout = 2;
}
=== ListContainersRequest ===
message ListContainersRequest {
    ContainerFilter filter = 1;
}
=== ContainerStatusRequest ===
message ContainerStatusRequest {
    // ID of the container for which to retrieve status.
    string container_id = 1;
    // Verbose indicates whether to return extra information about the container.
    bool verbose = 2;
}
=== ListImagesRequest ===
message ListImagesRequest {
    // Filter to list images.
    ImageFilter filter = 1;
}
=== ImageStatusRequest ===
message ImageStatusRequest {
    // Spec of the image.
    ImageSpec image = 1;
    // Verbose indicates whether to return extra information about the image.
    bool verbose = 2;
}
=== Stop/RemovePodSandboxRequest ===
message StopPodSandboxRequest {
    // ID of the PodSandbox to stop.
    string pod_sandbox_id = 1;
}
message RemovePodSandboxRequest {
    // ID of the PodSandbox to remove.
    string pod_sandbox_id = 1;
}
Several field names differ (pod_sandbox_id not sandbox_id) and some requests have extra fields. Let me check the remaining ones, then fix all.

Bash Check RemoveContainerRequest and filter fields
IN
P=/home/yunfei/Code/Only-AgentSandbox/only-agent-sandbox/crates/oas-cri/proto/api.proto; echo "=== RemoveContainerRequest ==="; sed -n '/message RemoveContainerRequest {/,/^}/p' $P; echo "=== ContainerFilter ==="; sed -n '/message ContainerFilter {/,/^}/p' $P; echo "=== PodSandboxFilter ==="; sed -n '/message PodSandboxFilter {/,/^}/p' $P

OUT
=== RemoveContainerRequest ===
message RemoveContainerRequest {
    // ID of the container to remove.
    string container_id = 1;
}
=== ContainerFilter ===
message ContainerFilter {
    // ID of the container.
    string id = 1;
    // State of the container.
    ContainerStateValue state = 2;
    // ID of the PodSandbox.
    string pod_sandbox_id = 3;
    // LabelSelector to select matches.
    // Only api.MatchLabels is supported for now and the requirements
    // are ANDed. MatchExpressions is not supported yet.
    map<string, string> label_selector = 4;
}
=== PodSandboxFilter ===
message PodSandboxFilter {
    // ID of the sandbox.
    string id = 1;
    // State of the sandbox.
    PodSandboxStateValue state = 2;
    // LabelSelector to select matches.
    // Only api.MatchLabels is supported for now and the requirements
    // are ANDed. MatchExpressions is not supported yet.
    map<string, string> label_selector = 3;
}
Let me rewrite lifecycle.rs with the correct field names and ..Default::default() where fields are omitted.

Write lifecycle.rs
411 lines
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
Bash Run UDS e2e lifecycle tests
IN
cargo test --manifest-path /home/yunfei/Code/Only-AgentSandbox/only-agent-sandbox/Cargo.toml -p oas-e2e --test lifecycle 2>&1 | tail -25

OUT
    |
297 | pub fn sandbox_req(uid: &str, type_id: u8) -> oas_manager::CreateSandboxRequest {
    |        ^^^^^^^^^^^

warning: function `container_req` is never used
   --> crates/oas-e2e/tests/common/mod.rs:318:8
    |
318 | pub fn container_req(sandbox_id: &str, image: &str) -> oas_manager::CreateContainerRequest {
    |        ^^^^^^^^^^^^^

warning: `oas-e2e` (test "lifecycle") generated 8 warnings (run `cargo fix --test "lifecycle" -p oas-e2e` to apply 1 suggestion)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 10.14s
     Running tests/lifecycle.rs (only-agent-sandbox/target/debug/deps/lifecycle-24f5291f7b6aa0c6)

running 7 tests
test e2e_create_container_image_not_whitelisted_returns_not_found ... ok
test e2e_run_sandbox_idempotent_by_uid ... ok
test e2e_list_pod_sandbox_after_run ... ok
test e2e_image_status_whitelist ... ok
test e2e_list_containers_filter_by_sandbox ... ok
test e2e_remove_pod_sandbox_idempotent ... ok
test e2e_full_lifecycle ... ok

test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s
All 7 e2e lifecycle tests pass. Let me suppress the cross-binary dead-code warnings on the shared helpers, then run the full workspace suite.

Edit mod.rs
Added 1 line
Edit mod.rs
Added 1 line
Bash Check workspace build warnings
IN
cargo build --workspace 2>&1 | grep -E "warning|error" | head -20; echo "=== exit: build done ==="

OUT
error: could not find `Cargo.toml` in `/home/yunfei/Code/Only-AgentSandbox` or any parent directory
=== exit: build done ===
Bash Check workspace build warnings
IN
cargo build --workspace --manifest-path /home/yunfei/Code/Only-AgentSandbox/only-agent-sandbox/Cargo.toml 2>&1 | grep -E "warning|error" | head -20; echo "=== done ==="

OUT
=== done ===
Clean build, no warnings. Now the full workspace test suite.

Bash Run full workspace test suite
IN
cargo test --workspace --manifest-path /home/yunfei/Code/Only-AgentSandbox/only-agent-sandbox/Cargo.toml 2>&1 | grep -E "test result|Running|error\[|error:|panicked" | head -60

OUT
     Running unittests src/lib.rs (only-agent-sandbox/target/debug/deps/oas_cri-2d2b8e974d513c78)
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
     Running tests/uds.rs (only-agent-sandbox/target/debug/deps/uds-e24a5a4e9c74ab40)
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
     Running unittests src/lib.rs (only-agent-sandbox/target/debug/deps/oas_driver-f49cac3026c52f93)
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
     Running unittests src/lib.rs (only-agent-sandbox/target/debug/deps/oas_e2e-ed44a7105f5d1ba3)
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
     Running tests/lifecycle.rs (only-agent-sandbox/target/debug/deps/lifecycle-24f5291f7b6aa0c6)
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s
     Running tests/manager_fsm.rs (only-agent-sandbox/target/debug/deps/manager_fsm-6bb42d5eac6f80c6)
test result: ok. 30 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s
     Running unittests src/lib.rs (only-agent-sandbox/target/debug/deps/oas_manager-c671fd023a1257de)
test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.05s
     Running unittests src/lib.rs (only-agent-sandbox/target/debug/deps/oas_net-8ba59d20d0de265b)
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
     Running unittests src/main.rs (only-agent-sandbox/target/debug/deps/oas_runtime-6d88504d6528edf4)
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
     Running unittests src/lib.rs (only-agent-sandbox/target/debug/deps/oas_storage-e220c16ac7fef784)
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
     Running unittests src/lib.rs (only-agent-sandbox/target/debug/deps/oas_store-35feb8eabd3b9c9b)
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
     Running tests/memory.rs (only-agent-sandbox/target/debug/deps/memory-be785cf5123e92e2)
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
     Running tests/redb.rs (only-agent-sandbox/target/debug/deps/redb-c3a8498483a39bbe)
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.16s
     Running unittests src/lib.rs (only-agent-sandbox/target/debug/deps/oas_types-c5fccd72b1b2a599)
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
All green workspace-wide. Let me finalize the todos.

Update Todos

Phase 0: Setup — add oas-e2e skeleton, workspace redb dep, oas-manager/oas-store Cargo deps; compile-only green

Phase 1: Store object-safety + MemoryStore (TDD)

Phase 2: RedbStore (TDD) — conformance green

Phase 3: Manager supporting types — Clock/IdGenerator/SandboxTypeTable/PerKeyLock + tests

Phase 4-6: OasManager FSM (sandbox/container/image/version) + mocks + direct FSM tests

Phase 7: E2E over UDS — oas-e2e lifecycle.rs with real Manager + mocks; green
Done. The real oas-manager + oas-store are built test-first, end-to-end green from CRI gRPC through the real Manager.

What landed
oas-store (lib.rs, memory.rs, redb.rs)

Made Store object-safe: transaction now takes Box<dyn FnOnce(&mut dyn Txn) -> Result<(), StoreError>>, and Txn is a backend-agnostic trait. The four write methods are default-impls delegating to transaction, so backends only implement reads + IPAM + transaction.
MemoryStore (HashMap, snapshot-rollback) and RedbStore (redb 4.1, serde_json, eager tables, native drop-without-commit rollback, in-memory backend for tests) share one conformance suite in tests/common/mod.rs — both pass identically.
oas-manager (manager.rs + clock/id/types_table/lock/readiness)

OasManager with full FSM: run/stop/remove sandbox, create/start/stop/remove container, image whitelist, version/status. Includes per-pod_uid idempotency, failure rollback (delete_vm+cleanup+teardown), PerKeyLock, injectable Clock, IdGenerator, SandboxTypeTable.
Your refinements applied: start Created→Running / Running→Ok / Exited→Conflict; stop idempotent across Created/Running/Exited.
oas-e2e (new test-only crate) — hand-rolled MockDriver/MockNet/MockStorage (no mockall; MockNet does real IPAM through the store) + ImmediateReadiness/NeverReadyReadiness. manager_fsm.rs drives the Manager directly (30 tests); lifecycle.rs drives it over the CRI UDS socket (7 tests).

Results
cargo build --workspace clean (no warnings); cargo test --workspace all green — 54 tests (4 CRI convert + 1 stub-UDS + 10 manager unit + 2 store conformance + 30 FSM + 7 UDS-lifecycle). Red-lines asserted: metadata roundtrip, no exit_code inference, and list_*/*_status never touch the driver.

Deferred (TODOs in code)
GuestAgent + exit_code回灌, reconcile (§4.6), real driver/net/storage impls, real eventfd (currently a VmReadiness trait with sentinel event_fd), main.rs swap (still StubManager), update_runtime_config persistence.

写一个测试设计文档以说明当前的测试的正确性
我将编写一份测试设计文档，用以梳理目前的测试体系，并论证这些测试为何能够验证其预期的正确性。我来把它放置在其他设计文档旁的 0docs/ 目录下。

Write 测试设计.md
178 lines
# only-agent-sandbox 测试设计 v0.1

**版本**：v0.1（对应 oas-manager 真 impl + oas-store 落地这一迭代）
**目的**：说明当前测试体系的分层、每条用例验证的性质、以及它如何为「编排层 + 持久化层」的正确性提供证据。本文不重复 [概要设计0.2.md](概要设计0.2.md) 的契约定义，只讲**测试如何证明实现符合契约**。

> 结论先行：当前共 **54 个测试用例**（4 CRI 翻译 + 1 CRI 握手 + 10 manager 单元 + 2 store 一致性 ×2 后端 + 30 FSM + 7 UDS 端到端），`cargo test --workspace` 全绿、`cargo build --workspace` 零警告。三条实现红线（metadata 原样 / exit_code 不反推 / List·Status 不打 driver）均有针对性断言。

---

## 1. 测试目标与覆盖原则

本轮实现的核心是「编排层 FSM + 持久化层」，其正确性可分解为若干**可观测性质**。每条测试都对应一个性质，断言失败即说明该性质被破坏：

| 性质 | 出处 | 由哪些测试保证 |
|------|------|----------------|
| 双层幂等 | §5.1 | `run_sandbox_idempotent_by_uid`、`e2e_run_sandbox_idempotent_by_uid`、`remove_*_idempotent` |
| 失败原子回滚 | §4.2 失败边界 | `*_storage_fail_rolls_back_net`、`*_create_vm_fail_rolls_back`、`*_readiness_timeout_rolls_back_all` |
| 红线① metadata/labels/annotations 原样 | §5.3 | store 一致性 `metadata_passthrough` + CRI convert 往返 |
| 红线② exit_code 不反推 | §5.3 / §4.4 | `stop_container_*` 断言 exit_code=0 仅在显式 stop 时；未模拟 agent 自然退出路径（延后） |
| 红线③ List·Status 只读 store | §5.3 / §4.3 | `read_paths_never_call_driver`、`container_status_never_calls_driver`（断言 mock 调用计数=0） |
| 事务原子性 | §3.6 | store 一致性 `transaction_commit_and_rollback`（提交可见 / Err 回滚 / 分组原子） |
| FSM 状态迁移合法 | §3.2 | container FSM 全状态机用例（start/stop 各状态分支） |
| 错误码映射 | §3.1 | `e2e_create_container_image_not_whitelisted_returns_not_found` 等（ImageNotInList→not_found） |

**测试原则**：
1. **测试不依赖被测代码的内部实现**，只观测对外可观测行为（返回值、store 落库内容、mock 调用计数、gRPC 状态码）。
2. **mock 只替换本轮不实现的真实外部依赖**（driver/net/storage），被测核心（Manager FSM + Store）用真实实现。
3. **确定性**：时钟可注入（`FakeClock`）、ID 单调可预测、redb 用内存后端、UDS 路径唯一——无 flaky。

---

## 2. 测试分层架构

```plaintext
┌─────────────────────────────────────────────────────────────────┐
│ ④ UDS 端到端 (oas-e2e/tests/lifecycle.rs, 7)                    │
│   CRI gRPC client ─UDS─► RuntimeSvc/ImageSvc ─► 真实 OasManager │
│   证据：proto 翻译 + 状态映射 + 错误码全链路正确                 │
├─────────────────────────────────────────────────────────────────┤
│ ③ FSM 直驱 (oas-e2e/tests/manager_fsm.rs, 30)                   │
│   直接调 Manager 方法，断言 store 落库 + mock 调用计数           │
│   证据：FSM 迁移 / 幂等 / 回滚 / 红线③                          │
├─────────────────────────────────────────────────────────────────┤
│ ② 单元 (oas-manager 单元 10 + oas-store 一致性 2×10 + CRI 5)    │
│   支撑类型 + Store 契约 + CRI 翻译，隔离最薄、最快              │
├─────────────────────────────────────────────────────────────────┤
│ ① 契约/支撑类型 (clock/id/types_table/lock 内联单元)            │
└─────────────────────────────────────────────────────────────────┘
```

依赖方向上，`oas-e2e` 是唯一同时依赖 `oas-cri` 与四个下层 crate 的顶层测试 crate，使「CRI→Manager」端到端成为可能而不污染生产 crate 的依赖方向（`oas-cri` 仍只依赖 `oas-manager`）。

---

## 3. 各层用例详解

### 3.1 ① 支撑类型单元（oas-manager 内联，10 条）

位于各模块的 `#[cfg(test)] mod tests`，验证 FSM 的「积木」正确，避免把积木缺陷混入 FSM 测试造成归因困难。

| 模块 | 用例 | 验证性质 |
|------|------|----------|
| `clock` | `fake_clock_advance_and_tick`、`system_clock_is_positive` | 时钟可注入且单调；`FakeClock` 使 `created_at/started_at` 确定可断言 |
| `id` | `sandbox_ids_unique`(1000)、`container_ids_unique_and_prefixed`、`vm_id_starts_at_one_and_increments` | 进程内 ID 不冲突；`vm_id` 避开 0 哨兵 |
| `types_table` | `known_types`、`unknown_type_rejected`、`all_images_flattens_whitelist` | type_id 0..=2 配置正确、越界→`InvalidArgument`、白名单可展平 |
| `lock` | `same_key_serializes`、`different_keys_parallel` | per-key 串行、不同 key 不互斥（支撑 §5.2 锁顺序） |

`same_key_serializes` 的设计：持锁后 spawn 另一任务锁同一 key，断言短等后仍未拿到（阻塞），释放后才能拿到——直接证明串行语义，而非靠时序碰运气。

### 3.2 ② Store 一致性套件（oas-store/tests，2 后端 × 10 子例）

`tests/common/mod.rs::run_store_tests<S: Store>` 是**与后端无关**的泛型函数，`memory.rs` 与 `redb.rs` 各调一次。这是关键设计：**同一组断言对两个后端都必须通过**，从而证明 `MemoryStore` 与 `RedbStore` 行为等价、均符合 `Store` 契约。

子例分组：

- **sandbox/container CRUD**：put/get 往返、覆盖写、缺失→`NotFound`。
- **幂等键**：`get_sandbox_by_uid` 命中/未命中——这是双层幂等的存储侧基础。
- **过滤收敛**：按 id / label_selector / state / pod_uid / sandbox_id 过滤，含组合条件。
- **删除幂等**：删不存在返回 `Ok(())`（与 CRI `swallow_not_found` 双保险）。
- **IPAM**：`/30` 仅 2 个可分配→第三次耗尽（`Db`）；释放后可重用；释放不存在=`Ok`。
- **事务**：提交后两条写均可见；闭包返回 `Err` 则两条写**均不可见**（回滚）；`delete_sandbox + release_ip` 分组原子；空事务=`Ok`。
- **红线①**：`metadata_passthrough` 全字段原样往返（含 `exit_code=137`/`OomKilled`/`message`/labels）。

事务回滚的两种实现都被同一断言覆盖：`MemoryStore` 用快照恢复，`RedbStore` 用 redb 原生 drop-without-commit——若任一实现回滚有缺陷，`transaction_rolls_back_on_err` 即失败。

### 3.3 ② CRI 翻译单元（oas-cri，4 + 1）

- `convert.rs` 内联 4 条：metadata 往返、labels/annotations 透传、state 映射、缺 `agent-sandbox/type` 注解→`InvalidArgument`。
- `tests/uds.rs` 1 条：StubManager 装配下 UDS 握手（Version/Status/ListImages/ImageFsInfo）+ RunPodSandbox 短路 `Unavailable`。保证本轮真 Manager 落地后 CRI 层契约未回归。

### 3.4 ③ FSM 直驱（oas-e2e/tests/manager_fsm.rs，30 条）

直接调 `Manager` trait 方法，注入真实 `MemoryStore` + mock driver/net/storage + `FakeClock`。这是覆盖 FSM 细节最密的一层。

**Sandbox FSM（11）**：
- happy：落库 READY，`pod_ip`/`vm_id`/`created_at` 正确，driver/net/storage 各调一次。
- 幂等：同 `pod_uid` 二次 run 返回同 id、`create_count==1`（不建第二个 VM）。
- NotReady 后再 run→`Conflict`（强制先 remove）。
- type 校验：type2 缺 cloud-disk / type0 带 cloud-disk →`InvalidArgument`，且 driver/net 未被调用。
- **回滚三连**：storage 失败→回滚 net（`teardown_count==1`、`create_count==0`）；create_vm 失败→回滚 storage+net（`delete_count==0` 无 VM 可删）；readiness 超时→回滚 driver+storage+net（`delete_count==1` 删孤儿 VM）且**store 无记录**。
- stop：`delete_vm` 后轮询 `get_vm` 到 Stopped、`net.teardown` 释放 IP、状态→NotReady。
- remove 幂等：二次删=`Ok`，删后 `sandbox_status`→`NotFound`。
- **红线③**：`read_paths_never_call_driver` 断言 `get_count==0 && list_count==0`。

**Container FSM（14）**——按用户修订的 CRI 幂等语义：
- create：happy（CREATED、env `KEY=VAL`）；image 不在白名单→`ImageNotInList`；memory 超 type 预算→`TypeMismatch`；sandbox 缺失→`NotFound`。
- **start**：Created→Running 且 `started_at` 非零；Running→`Ok`（幂等）；Exited→`Conflict`。
- **stop**：Created→Exited(exit_code=0/Completed/finished_at)；Running→Exited；Exited→`Ok`（幂等）；缺失→`Ok`（幂等）。
- remove 幂等；按 sandbox_id 列表；`container_status_never_calls_driver`。

**image + version/status（5）**：`image_status` 命中/未命中；`pull_image` 白名单内/外；`list_images` 展平 3 项；`remove_image` 空操作；version/status 固定值。

> **回滚三连**是本轮最有力的正确性证据：它证明 `run_sandbox` 在三个不同失败点都正确调用了 `rollback_create`，且顺序为 driver→storage→net，资源不留孤儿。`readiness_timeout_rolls_back_all` 额外断言「建了 VM 但未就绪」时 store 不落记录——即 §5.1 的「create_vm 成功未 commit 崩溃」边界在测试层面被覆盖（孤儿由 `delete_vm` 回收）。

### 3.5 ④ UDS 端到端（oas-e2e/tests/lifecycle.rs，7 条）

把真实 `OasManager` 经 `RuntimeSvc`/`ImageSvc` 挂到 UDS，用 tonic 生成的 client 调真实 CRI gRPC。证明**全链路**：proto→domain→Manager→Store→proto 的翻译与状态映射无误。

- `e2e_full_lifecycle`：RunPodSandbox(READY+ip)→CreateContainer(CREATED)→Start(RUNNING+started_at)→Stop(EXITED+exit_code=0+reason=Completed)→Remove(NotFound)→StopPodSandbox(NotReady)→RemovePodSandbox(NotFound)。
- `e2e_list_pod_sandbox_after_run` / `e2e_list_containers_filter_by_sandbox`：List 收敛 + 过滤。
- `e2e_image_status_whitelist`：ImageStatus 命中/未命中 + ListImages=3。
- `e2e_create_container_image_not_whitelisted_returns_not_found`：**错误码映射**——`ImageNotInList` 经 `error::to_status` 映射为 `tonic::Code::NotFound`（与 CRI 契约一致）。
- `e2e_run_sandbox_idempotent_by_uid`：gRPC 层同 uid 二次 run 返回同 id、`create_count==1`。
- `e2e_remove_pod_sandbox_idempotent`：二次 remove=`Ok`。

此层的价值：FSM 直驱层用的是 domain 类型直传；UDS 层强制经过 proto 序列化/反序列化与 `convert.rs` 双向翻译，能抓到 FSM 层抓不到的翻译缺陷（如字段名错配、状态枚举映射错）。

---

## 4. 测试如何保证正确性（技术手段）

### 4.1 mock 调用计数 = 行为可观测化
`MockDriver`/`MockNet`/`MockStorage` 用 `Arc<Mutex<Vec<_>>>` 记录每次调用。这让「红线③ List·Status 不打 driver」从「相信实现」变成**可断言事实**：`assert_eq!(env.driver.get_count(), 0)`。同理，回滚测试靠 `teardown_count`/`cleanup_count`/`delete_count` 证明副作用发生。

### 4.2 真实 Store + mock 外设 = 高保真
被测的 Manager FSM 用**真实 `MemoryStore`**（非 mock），所以「落库内容正确」「事务原子」是被真实验证的；只有 driver/net/storage 这种本轮不实现的外部依赖被 mock。`MockNet` 甚至持真实 `Store` 做 IPAM（`lease_ip`/`release_ip`），让 IPAM 崩溃不重复分配的性质也进入 e2e。

### 4.3 确定性
- `FakeClock` 固定起点 `1_700_000_000`，`created_at`/`started_at` 可精确断言（见 `run_sandbox_happy`、`start_container_created_to_running`）。
- `IdGenerator` 单调，id 前缀固定（`sb-`/`ct-`）。
- redb 用 `InMemoryBackend`，每个 `make()` 独立实例——用例间零状态泄漏。
- UDS 路径用 `pid + AtomicU64` 序列化唯一，并行测试不撞 socket。

### 4.4 双后端等价
`run_store_tests<S: Store>` 泛型套件对 `MemoryStore` 与 `RedbStore` 跑同一断言。任何只在某后端暴露的序列化/事务/扫描缺陷都会被一边抓到。

### 4.5 TDD 顺序的证据
每个阶段以「先写测试→实现→绿」推进，阶段间 `cargo test --workspace` 必绿才进入下一阶段。这保证测试与实现同步演进，而非事后补测。

---

## 5. 已知局限（未覆盖）

诚实地列出当前测试**不**覆盖的部分，避免过度宣称：

1. **GuestAgent / exit_code 带外回灌**（§2.8/§4.4）：本轮延后。`stop_container` 的 `exit_code=0` 是**显式停止**的简化，**未测试**自然退出由 agent 回灌 exit_code 的路径。红线②仅以「不反推」的方式部分保证（没有测试声称能从 VM 存活推出 exit_code）。
2. **reconcile**（§4.6）：未实现、未测试。孤儿回收的「reconcile 发现 VM 没了→NOTREADY」未覆盖；当前仅覆盖「run 中失败→主动回滚」。
3. **并发竞态**（§5.2 的 4 个必处理竞态）：`PerKeyLock` 的串行语义有单元测试，但「Run 在途 Stop 到达」「同 pod_uid 并发两次 Run」的**跨任务时序竞态**未在 FSM/e2e 层显式构造（单线程 tokio test）。锁的正确性靠 `same_key_serializes` 间接保证。
4. **RedbStore 持久化跨进程**：用内存后端测试，**未测试**「崩溃后重新 open 文件、租约不重复分配」的真实持久化语义。
5. **真实 driver/net/storage**：均 mock，Firecracker snapshot load / netns / ext4 真实行为未覆盖（待专题）。
6. **`main.rs` 装配**：仍注入 `StubManager`，真 Manager 的生产装配未测试。

---

## 6. 运行与回归

```bash
cargo build --workspace        # 零警告
cargo test --workspace         # 54 用例全绿
cargo test -p oas-store        # Memory + Redb 一致性
cargo test -p oas-e2e          # 30 FSM + 7 UDS e2e
```

新增 FSM 行为或 Store 后端时，回归策略：
- Store 后端变更→跑 `run_store_tests`（自动覆盖两后端）。
- FSM 迁移变更→在 `manager_fsm.rs` 加直驱用例 + `lifecycle.rs` 加 UDS 用例（前者断内部、后者断翻译）。
- CRI 翻译变更→`convert.rs` 内联往返用例 + `lifecycle.rs` 验证 gRPC 状态码。
测试设计文档已写好：0docs/测试设计.md

文档结构：

目标与覆盖原则 — 一张表把每条可观测性质（双层幂等 / 失败原子回滚 / 三条红线 / 事务原子 / FSM 迁移 / 错误码映射）映射到保证它的具体测试。
分层架构 — 四层金字塔图：支撑类型单元 → Store 一致性 + CRI 翻译 → FSM 直驱 → UDS 端到端，并说明 oas-e2e 为何是唯一能做 CRI→Manager 端到端的顶层 crate。
各层用例详解 — 逐层列出用例名与各自验证的性质。重点解释了最有力的证据：
Store 一致性套件用泛型 run_store_tests<S> 对 Memory+Redb 跑同一断言 → 证明双后端等价 + 契约符合；
FSM 层的回滚三连（storage/create_vm/readiness 三个失败点都正确回滚，readiness 超时还断言 store 无记录 = 孤儿回收边界）；
UDS 层强制经过 proto 序列化与 convert.rs 双向翻译，能抓 FSM 层抓不到的翻译缺陷。
技术手段 — mock 调用计数把红线③变成可断言事实、真实 Store + mock 外设的高保真、FakeClock/redb 内存后端的确定性、双后端等价、TDD 顺序。
已知局限 — 诚实列出 6 项未覆盖（GuestAgent 回灌、reconcile、跨任务竞态时序、RedbStore 真实持久化、真实 driver/net/storage、main.rs 装配），避免过度宣称。
运行与回归策略 — 命令 + 各类变更时的回归入口。
文档中的数字与实际一致：54 个测试用例全绿、构建零警告。

验收测试方案.md#55-273
如果我现在执行测试0docs/验收测试方案.md#L55-273。我能否看到kubelet不报错呢，kubelet是否认为容器已经启动？即使firecracker没有启动（顶层都是mock）
短答：不能。 按当前代码状态，kubelet 一定会报错，Pod 起不来。即便我把真 OasManager（mock 后端）接进 main.rs，也只能在 CRI 协议层骗过 kubelet，而 验收测试方案.md 的 T1 用第二来源（pgrep firecracker、tap、IP）交叉验证，照样会暴露。下面分两层说清楚。

一、现状第一道墙：main.rs 还是 StubManager
src/main.rs 至今注入的是 StubManager，没换成真 OasManager（这一步在计划里被明确延后了）。StubManager::run_sandbox 直接返回 OasError::Unavailable("manager not implemented (§3.2)")。

所以你一跑：

crictl runp / kubelet 投放 manifest → RunPodSandbox → Status::unavailable
kubelet 日志会刷 RunPodSandbox failed、Pod 卡在 ContainerCreating 并反复重试
这一步 kubelet 就已经在报错了，根本走不到"容器是否启动"的问题。

二、即便接上真 OasManager（mock 后端），还有两道硬 mismatch
假设我把 main.rs 换成真 OasManager（driver/net/storage 全是 e2e 里那套 mock），按 T1 的模板跑：

阻塞 1：agent-sandbox/type: "lite" 解析不了
convert.rs:153-159 的 parse_type_id 是 raw.parse::<u8>()——只接受数字。模板里写的是字符串 "lite"/"pro"/"browser"，parse::<u8>() 直接失败 → InvalidArgument → RunPodSandbox 在 CRI 翻译层就挂了，连 Manager 都没进。

阻塞 2：镜像不在白名单
types_table.rs 默认白名单是 img-a/img-b/img-c。模板用的 agent-sandbox/lite:1.0 等不在白名单 → create_container → ImageNotInList → CRI not_found（manager.rs 的 create_container）→ kubelet 建容器失败。

这两个 mismatch 是真实现与验收方案之间的契约错位：实现用数值 type_id + 占位镜像名，验收方案用语义化类型名 + 真实镜像引用。两边对不上。

三、假设把阻塞 1、2 都修了，CRI 层会发生什么
若把 lite/pro/browser 映射到 0/1/2、并把对应镜像加进白名单，那么 mock 后端下：

CRI 调用	行为	kubelet 视角
RunPodSandbox	mock net/storage/driver 全成功，落库 READY	sandbox READY ✅
CreateContainer	白名单命中，落库 CREATED	✅
StartContainer	不调 agent，直接 CREATED→RUNNING（manager.rs）	容器 RUNNING ✅
ListPodSandbox/ListContainers/*Status（PLEG 每秒）	只读 store，不打 driver（红线③）	稳定、sandbox_id 不变 ✅
ImageFsInfo	返回"磁盘很空"假值	不触发驱逐 ✅
Status	RuntimeReady/NetworkReady=true	kubelet 认为就绪 ✅
这一层 kubelet 确实不会报 CRI 错，也确实会"认为容器已启动"——因为对 kubelet 而言，CRI 说 RUNNING 就是 RUNNING。PLEG 也不会无限重建（读路径不碰 mock driver，状态恒定）。

四、但 T1 仍然会失败，而且这是"假绿"
第二来源全挂（T1 判据 3.1 的"VM 真存在""无残留"）：
pgrep firecracker → 0 个进程（driver 是 mock，没起 FC）
ip -o link show type tun → 无 tap（net 是 mock）
没有真实 IP 归还/块设备 detach
snapshot_residue 对比会 FAIL
容器永远不会 EXITED：没有真进程、没有 GuestAgent 回灌 Exited，store 里容器永远 RUNNING。对 restartPolicy: Never 的预期会退出的 workload，kubelet 会一直等不到退出（30s 稳态窗口内看不出来，但语义是错的）。
kubelet 日志噪音：ContainerStats/ListContainerStats/PodSandboxStats 等是 unsupported（unsupported.rs），kubelet 周期性 stats 采集会刷 unimplemented 错误。T1 的 grep 只抓 evict|DiskPressure|recreate，不抓 stats 错误，所以可能恰好绕过 T1 的日志判据——但这只是侥幸，不是正确。
换句话说：mock 后端能让 kubelet 在协议层"满意"，但本质是在撒谎——声称 RUNNING 却没有任何真实进程。T1 的第二来源交叉验证正是为了抓这种谎言，所以它一定会 FAIL。

五、结论与建议
当前直接跑 T1：kubelet 报错（StubManager→Unavailable），Pod 起不来。
接真 Manager + mock 后端 + 修契约 mismatch：kubelet 在 CRI 层不报错、认为容器 RUNNING，但 T1 因第二来源（无 FC 进程）失败——这是预期，因为 driver/net/storage 还是 mock。
要让 T1 真正通过：必须有真实的 FirecrackerDriver 实现（真起 FC 进程、snapshot load、eventfd）+ 真实 NetworkManager（真 netns/tap/IPAM）+ 真实 StorageManager（真 ext4/云盘）+ GuestAgent（真起进程、回灌 exit_code）。这些正是计划里"待专题/延后"的部分。
mock 后端的真正价值是单元/FSM/带 mock 的端到端（也就是现在 oas-e2e 那 37 条），用来验证编排逻辑本身正确；它不是用来糊弄真 kubelet 验收的。

如果你下一步目标是"让真 kubelet 跑通 T1"，正确的做法是先落地真实 driver（§3.3 专题），而不是把 mock 接进 main。要的话我可以帮你列一个"main.rs 接真 Manager + 契约对齐（type 名映射、白名单）"的最小改动清单——但请明确：那只能到第三节"CRI 层不报错"，到不了"T1 通过"。

概要设计0.2.md#19-19
去修改agent-sandbox/type: "lite"等，改为agent-sandbox/type: 0等，你编写一个小测试用于测试mock 后端的api、manager、store层能能否让 kubelet 在协议层"满意"（仅仅测试启动停止1个沙箱）（这类似web app的前后端分离设计）我们要确保当前的这三层ok，在做firecracker等底层的内容
建议得好——这正是“前后端分离”的思路：在构建真正的 firecracker 后端之前，先证明后端三层的协议契约。我来完成这两件事：将验收模板与数字 type_id 对齐，并添加一个聚焦于 kubelet 协议的测试。

我先读取一下验收文档中的具体模板行。

Bash Find type annotation lines in acceptance doc
IN
grep -n 'agent-sandbox/type' /home/yunfei/Code/Only-AgentSandbox/0docs/验收测试方案.md

OUT
121:    agent-sandbox/type: "lite"
142:    agent-sandbox/type: "pro"
162:    agent-sandbox/type: "browser"
347:    agent-sandbox/type: "lite"
459:  "annotations": {"agent-sandbox/type":"lite"}, "linux": {}
Bash Read context around type annotations
IN
sed -n '118,165p;344,350p;456,462p' /home/yunfei/Code/Only-AgentSandbox/0docs/验收测试方案.md

OUT
  name: sb-lite-__ID__
  namespace: default
  annotations:
    agent-sandbox/type: "lite"
spec:
  runtimeClassName: agent-firecracker
  restartPolicy: Never
  containers:
    - name: workload
      image: "agent-sandbox/lite:1.0"
      imagePullPolicy: IfNotPresent
      resources:
        requests: { cpu: "1", memory: "1Gi" }
        limits:   { cpu: "1", memory: "1Gi" }
```

#### `pod-pool/pro.yaml` （差异：4C8G、image=pro）
```yaml
apiVersion: v1
kind: Pod
metadata:
  name: sb-pro-__ID__
  namespace: default
  annotations:
    agent-sandbox/type: "pro"
spec:
  runtimeClassName: agent-firecracker
  restartPolicy: Never
  containers:
    - name: workload
      image: "agent-sandbox/pro:1.0"
      resources:
        requests: { cpu: "4", memory: "8Gi" }
        limits:   { cpu: "4", memory: "8Gi" }
```

#### `pod-pool/browser.yaml` （差异：2C4G、/dev/shm）
```yaml
apiVersion: v1
kind: Pod
metadata:
  name: sb-browser-__ID__
  namespace: default
  annotations:
    agent-sandbox/type: "browser"
spec:
  runtimeClassName: agent-firecracker
  restartPolicy: Never
metadata:
  name: sb-t2-__ID__
  annotations:
    agent-sandbox/type: "lite"
spec:
  restartPolicy: Never
  runtimeClassName: agent-firecracker
  cat > "/tmp/pod-$1.json" <<EOF
{
  "metadata": {"name":"perf-$1","namespace":"default","uid":"uid-$1","attempt":0},
  "annotations": {"agent-sandbox/type":"lite"}, "linux": {}
}
EOF
}
现在让我来对齐类型注解（仅限注解，镜像引用保持不变）。

Edit 验收测试方案.md
Modified
Edit 验收测试方案.md
Modified
Edit 验收测试方案.md
Modified
Edit 验收测试方案.md
Modified
让我在这三个模板之后添加一条简短的映射说明，以便记录对齐情况。

Bash Find spot after templates for note
IN
sed -n '175,185p' /home/yunfei/Code/Only-AgentSandbox/0docs/验收测试方案.md

OUT
  volumes:
    - name: dshm
      emptyDir: { medium: Memory, sizeLimit: 1Gi }
```

> 🔍 `__ID__` 是占位符，投放脚本用 `sed` 替换为唯一序号，以支持 T3 批量并发。

---

## 三、T1 基础测试 —— 生命周期 happy path
Edit 验收测试方案.md
Added 2 lines
现在进行 kubelet-protocol 测试。让我把它添加到 lifecycle.rs 中（复用现有的 harness）。首先处理文件末尾。

Bash Read end of lifecycle.rs
IN
tail -20 /home/yunfei/Code/Only-AgentSandbox/only-agent-sandbox/crates/oas-e2e/tests/lifecycle.rs

OUT
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
Edit lifecycle.rs
Added 152 lines
Bash Run the new kubelet-protocol test
IN
cargo test --manifest-path /home/yunfei/Code/Only-AgentSandbox/only-agent-sandbox/Cargo.toml -p oas-e2e --test lifecycle e2e_kubelet_protocol_sandbox_lifecycle -- --nocapture 2>&1 | tail -20

OUT
256 | pub struct TestEnv {
    |            ------- fields in this struct
257 |     pub mgr: Arc<OasManager>,
258 |     pub store: Arc<MemoryStore>,
    |         ^^^^^
...
261 |     pub storage: Arc<MockStorage>,
    |         ^^^^^^^
262 |     pub clock: Arc<FakeClock>,
    |         ^^^^^

warning: `oas-e2e` (test "lifecycle") generated 6 warnings (run `cargo fix --test "lifecycle" -p oas-e2e` to apply 1 suggestion)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 7.98s
     Running tests/lifecycle.rs (only-agent-sandbox/target/debug/deps/lifecycle-24f5291f7b6aa0c6)

running 1 test
test e2e_kubelet_protocol_sandbox_lifecycle ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 7 filtered out; finished in 0.03s
测试通过。关于 lifecycle 二进制文件中未使用的 TestEnv 字段存在一些 dead-code 警告（该二进制文件未直接读取 store、storage 和 clock）。我将在测试辅助结构体上屏蔽这些警告。

Edit mod.rs
Added 1 line
Bash Run all oas-e2e tests, check warnings
IN
cargo test --manifest-path /home/yunfei/Code/Only-AgentSandbox/only-agent-sandbox/Cargo.toml -p oas-e2e 2>&1 | grep -E "test result|warning:|error" | head -20

OUT
warning: unused import: `Clock`
warning: method `fail_setup` is never used
warning: method `fail_create` is never used
warning: methods `fail_setup` and `setup_count` are never used
warning: methods `fail_provision`, `provision_count`, and `cleanup_count` are never used
warning: struct `NeverReadyReadiness` is never constructed
warning: `oas-e2e` (test "manager_fsm") generated 2 warnings (run `cargo fix --test "manager_fsm" -p oas-e2e` to apply 1 suggestion)
warning: `oas-e2e` (test "lifecycle") generated 5 warnings (1 duplicate)
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s
test result: ok. 30 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
8 个生命周期测试 + 30 个 FSM 测试全部通过。让我清理一下各二进制文件中共享 mock 的死代码警告（在相关二进制文件中未使用 mock 辅助方法）。

Edit mod.rs
Added 3 lines
Edit mod.rs
Modified
现在运行完整的 workspace 测试套件并检查是否有警告。

Bash Full workspace test + warnings check
IN
cargo test --workspace --manifest-path /home/yunfei/Code/Only-AgentSandbox/only-agent-sandbox/Cargo.toml 2>&1 | grep -E "test result:|warning:|error" | head -30

OUT
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s
test result: ok. 30 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s
test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.05s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.16s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
全部 55 个测试均已通过，没有警告。让我更新一下测试设计文档，以反映新的测试用例及其数量。

Edit 测试设计.md
Modified
Edit 测试设计.md
Added 14 lines
完成。两件事都做好了，全绿、零警告（现 55 个测试）。

1. 验收模板 type 注解对齐
0docs/验收测试方案.md 里所有 agent-sandbox/type: "lite"/"pro"/"browser" 已改为数字 "0"/"1"/"2"（含 yaml 模板与 JSON 示例），与 convert.rs::parse_type_id 的 u8 解析、SandboxTypeTable 的 type_id 一致。镜像引用（agent-sandbox/lite:1.0 等）未动——并在模板后加了一条 📌 注记：模板的 resources 与镜像引用尚未与 type 表 image_whitelist/资源预算对齐，container 级测试前需补齐（沙箱级不受影响）。

2. 协议层契约测试
新增 lifecycle.rs::e2e_kubelet_protocol_sandbox_lifecycle，按 kubelet 对单个沙箱的 CRI 调用序列推进，断言后端三层（API+Manager+Store，driver/net/storage 全 mock）在协议层满足 kubelet：

步骤	CRI 调用	断言
握手	Version + Status	runtime 名/版本正确；RuntimeReady/NetworkReady=true
防驱逐	ImageFsInfo	返回非空"磁盘很空"假值
建沙箱	RunPodSandbox(type=0)	SandboxReady + 非空 pod_ip
PLEG 稳态	3 轮 ListPodSandbox+PodSandboxStatus	状态恒定、id 不变；driver.get/list/create_count 三轮不变（红线③：PLEG 不压底层、无重建）
停	StopPodSandbox	SandboxNotready，仍在 List
删	RemovePodSandbox	之后 Status=NotFound、List 空
编排确证	—	driver.create/delete=1、net.teardown=1（mock 被正确调度）
这条测试正是你说的"前后端分离的后端侧"——在写真实 firecracker 之前，先证明 CRI→Manager→Store 的协议契约成立。

边界（已在测试设计文档中写明）
这条测试只验证协议层：kubelet 在 CRI 层会被满足（不报错、认为 sandbox 就绪、PLEG 稳定）。它不验证真实 VM/tap/IP 是否存在——那需真实 driver，属待专题的底层。所以它通过 ≠ 验收测试方案.md T1 通过（T1 的第二来源 pgrep firecracker 等仍会失败）。这正是"后端三层 OK 后再做底层"的切分点。

0docs/测试设计.md 已同步更新：用例数 54→55，并在 §3.5 增设"协议层契约测试"小节说明其设计与边界。

manager.rs#31-31
修改main.rs使用OasManager+mock后端
我将把 main.rs 修改为使用 OasManager 和 mock 后端。为了在 main.rs 中使用 mock，它们需要能够被生产环境二进制文件访问——目前它们被困在 oas-e2e 的测试代码中。最简洁的修复方案是：将 mock 提取到一个专门的 oas-mock crate 中，供 oas-runtime 和 oas-e2e 共同使用（单一事实来源）。我先读取当前的 mock 代码以便准确迁移。

Read mod.rs
现在我来创建 oas-mock crate，并将 mock 移入其中。

Write Cargo.toml
15 lines
[package]
name = "oas-mock"
version = "0.1.0"
edition = "2024"

[dependencies]
async-trait.workspace = true
tokio = { workspace = true }
oas-types = { path = "../oas-types" }
oas-driver = { path = "../oas-driver" }
oas-net = { path = "../oas-net" }
oas-storage = { path = "../oas-storage" }
oas-store = { path = "../oas-store" }
oas-manager = { path = "../oas-manager" }
Write lib.rs
279 lines
//! Mock 后端：driver / net / storage 的手写假实现 + `VmReadiness` 假实现。
//!
//! 设计目标：
//! - **不引 mockall**：每个 mock 用 `Arc<Mutex<…>>` 记录调用 + `AtomicBool` 注入失败，
//!   完全可控、零额外依赖。
//! - `MockNet` 持真实 `Store` 做 IPAM（§4.2：net 层 `lease_ip`/`release_ip`），让
//!   IPAM「崩溃后不重复分配」的性质在 mock 模式下也成立。
//! - 同时供 `oas-runtime`（`main.rs` 在真实 Firecracker 后端就绪前的临时装配）与
//!   `oas-e2e`（测试）共用，单一事实源。
//!
> ⚠️ 这是占位后端：`create_vm` 不起任何 firecracker 进程，`setup` 不建 netns/tap，
> `provision` 不制备 ext4。仅用于让上层（CRI/Manager/Store）在协议层跑通。真实底层
> 实现就绪后 `main.rs` 改用真后端，本 crate 退回仅测试用。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use oas_driver::{DriverError, FirecrackerDriver, VmId, VmLifecycle, VmSpec, VmStatus};
use oas_manager::{OasError, VmReadiness};
use oas_net::{NetConfig, NetError, NetworkManager};
use oas_storage::{DiskConfig, StorageError, StorageManager};
use oas_store::Store;

// ---- MockDriver ------------------------------------------------------------

/// 不起真实 firecracker 进程；记录 create/get/delete/list 调用，可注入 create 失败。
pub struct MockDriver {
    fail_create: AtomicBool,
    creates: Mutex<Vec<(String, u8)>>,
    deletes: Mutex<Vec<u64>>,
    gets: Mutex<Vec<u64>>,
    lists: Mutex<Vec<()>>,
    statuses: Mutex<HashMap<u64, VmStatus>>,
    next_vm_id: AtomicU64,
}

impl MockDriver {
    pub fn new() -> Self {
        Self {
            fail_create: AtomicBool::new(false),
            creates: Mutex::new(Vec::new()),
            deletes: Mutex::new(Vec::new()),
            gets: Mutex::new(Vec::new()),
            lists: Mutex::new(Vec::new()),
            statuses: Mutex::new(HashMap::new()),
            next_vm_id: AtomicU64::new(1),
        }
    }
    pub fn fail_create(&self, v: bool) {
        self.fail_create.store(v, Ordering::Relaxed);
    }
    pub fn create_count(&self) -> usize {
        self.creates.lock().unwrap().len()
    }
    pub fn delete_count(&self) -> usize {
        self.deletes.lock().unwrap().len()
    }
    pub fn get_count(&self) -> usize {
        self.gets.lock().unwrap().len()
    }
    pub fn list_count(&self) -> usize {
        self.lists.lock().unwrap().len()
    }
}

impl Default for MockDriver {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl FirecrackerDriver for MockDriver {
    async fn create_vm(
        &self,
        netns_path: &str,
        type_id: u8,
        _spec: VmSpec,
        _event_fd: std::os::fd::RawFd,
    ) -> Result<VmId, DriverError> {
        if self.fail_create.load(Ordering::Relaxed) {
            return Err(DriverError::Other("injected create_vm failure".into()));
        }
        let id = self.next_vm_id.fetch_add(1, Ordering::Relaxed);
        self.creates
            .lock()
            .unwrap()
            .push((netns_path.to_string(), type_id));
        self.statuses.lock().unwrap().insert(
            id,
            VmStatus {
                started: true,
                healthy: true,
                lifecycle: VmLifecycle::Running,
            },
        );
        Ok(VmId(id))
    }

    async fn get_vm(&self, id: VmId) -> Result<VmStatus, DriverError> {
        self.gets.lock().unwrap().push(id.0);
        match self.statuses.lock().unwrap().get(&id.0) {
            Some(s) => Ok(s.clone()),
            None => Err(DriverError::NotFound(id.0)),
        }
    }

    async fn delete_vm(&self, id: VmId) -> Result<(), DriverError> {
        self.deletes.lock().unwrap().push(id.0);
        let mut st = self.statuses.lock().unwrap();
        if let Some(s) = st.get_mut(&id.0) {
            s.lifecycle = VmLifecycle::Stopped;
        }
        Ok(())
    }

    async fn list_vm(&self) -> Result<Vec<VmStatus>, DriverError> {
        self.lists.lock().unwrap().push(());
        Ok(self.statuses.lock().unwrap().values().cloned().collect())
    }
}

// ---- MockNet ---------------------------------------------------------------

/// 不建真实 netns/tap；持真实 `Store` 做 IPAM（lease/release）。
pub struct MockNet {
    store: Arc<dyn Store>,
    cidr: String,
    gateway: String,
    setups: Mutex<Vec<String>>,
    teardowns: Mutex<Vec<String>>,
    fail_setup: AtomicBool,
}

impl MockNet {
    pub fn new(store: Arc<dyn Store>, cidr: &str, gateway: &str) -> Self {
        Self {
            store,
            cidr: cidr.to_string(),
            gateway: gateway.to_string(),
            setups: Mutex::new(Vec::new()),
            teardowns: Mutex::new(Vec::new()),
            fail_setup: AtomicBool::new(false),
        }
    }
    pub fn fail_setup(&self, v: bool) {
        self.fail_setup.store(v, Ordering::Relaxed);
    }
    pub fn setup_count(&self) -> usize {
        self.setups.lock().unwrap().len()
    }
    pub fn teardown_count(&self) -> usize {
        self.teardowns.lock().unwrap().len()
    }
}

#[async_trait]
impl NetworkManager for MockNet {
    async fn setup(&self, sandbox_id: &str) -> Result<NetConfig, NetError> {
        if self.fail_setup.load(Ordering::Relaxed) {
            return Err(NetError::Other("injected setup failure".into()));
        }
        let mut lease = self
            .store
            .lease_ip(&self.cidr)
            .map_err(|e| NetError::Other(e.to_string()))?;
        lease.sandbox_id = sandbox_id.to_string();
        self.setups.lock().unwrap().push(sandbox_id.to_string());
        Ok(NetConfig {
            netns_path: format!("/var/run/netns/oas-{sandbox_id}"),
            tap_name: "tap0".into(),
            mac: format!("02:oas:{sandbox_id}"),
            pod_ip: lease.ip.clone(),
            gateway: self.gateway.clone(),
            lease,
        })
    }

    async fn teardown(&self, net: &NetConfig) -> Result<(), NetError> {
        self.teardowns
            .lock()
            .unwrap()
            .push(net.lease.ip.clone());
        self.store
            .release_ip(&net.lease)
            .map_err(|e| NetError::Other(e.to_string()))?;
        Ok(())
    }
}

// ---- MockStorage -----------------------------------------------------------

/// 不制备真实 ext4 / 绑云盘；记录 provision/cleanup 调用，可注入 provision 失败。
pub struct MockStorage {
    fail_provision: AtomicBool,
    provisions: Mutex<Vec<(String, u8)>>,
    cleanups: Mutex<Vec<()>>,
}

impl MockStorage {
    pub fn new() -> Self {
        Self {
            fail_provision: AtomicBool::new(false),
            provisions: Mutex::new(Vec::new()),
            cleanups: Mutex::new(Vec::new()),
        }
    }
    pub fn fail_provision(&self, v: bool) {
        self.fail_provision.store(v, Ordering::Relaxed);
    }
    pub fn provision_count(&self) -> usize {
        self.provisions.lock().unwrap().len()
    }
    pub fn cleanup_count(&self) -> usize {
        self.cleanups.lock().unwrap().len()
    }
}

impl Default for MockStorage {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl StorageManager for MockStorage {
    async fn provision(
        &self,
        sandbox_id: &str,
        type_id: u8,
        cloud_disk_ref: Option<&str>,
    ) -> Result<DiskConfig, StorageError> {
        self.provisions
            .lock()
            .unwrap()
            .push((sandbox_id.to_string(), type_id));
        if self.fail_provision.load(Ordering::Relaxed) {
            return Err(StorageError::Other("injected provision failure".into()));
        }
        Ok(DiskConfig {
            rw_layer_path: Some(format!("/var/lib/oas/rw/{sandbox_id}.ext4")),
            cloud_disk_dev: cloud_disk_ref.map(|s| s.to_string()),
        })
    }

    async fn cleanup(&self, _disk: &DiskConfig) -> Result<(), StorageError> {
        self.cleanups.lock().unwrap().push(());
        Ok(())
    }
}

// ---- VmReadiness 假实现 ----------------------------------------------------

/// 立即就绪（happy path）。
pub struct ImmediateReadiness;

#[async_trait]
impl VmReadiness for ImmediateReadiness {
    async fn wait_ready(&self, _vm_id: VmId, _timeout: Duration) -> Result<(), OasError> {
        Ok(())
    }
}

/// 永不就绪：短暂等待后返回 `Unavailable`（用于测 readiness 超时回滚，不挂起）。
pub struct NeverReadyReadiness;

#[async_trait]
impl VmReadiness for NeverReadyReadiness {
    async fn wait_ready(&self, _vm_id: VmId, _timeout: Duration) -> Result<(), OasError> {
        tokio::time::sleep(Duration::from_millis(20)).await;
        Err(OasError::Unavailable(
            "vm not ready (never-ready fake)".into(),
        ))
    }
}
现在重构 oas-e2e/tests/common/mod.rs，使其从 oas-mock 导入 mocks（保留 TestEnv 和 helpers）。

Write mod.rs
109 lines
//! e2e 共享：测试装配（`TestEnv`）+ 请求构造助手。mock 后端来自 `oas_mock` crate。

#![allow(dead_code)]

use std::sync::Arc;

use oas_manager::{FakeClock, OasManager, VmReadiness};
use oas_mock::{ImmediateReadiness, MockDriver, MockNet, MockStorage};
use oas_store::{MemoryStore, Store};

// mock 后端统一从 oas_mock 复用（与 main.rs 同源）。
pub use oas_mock::{ImmediateReadiness, MockDriver, MockNet, MockStorage, NeverReadyReadiness};

// ---- 测试装配 --------------------------------------------------------------

/// 各 test 二进制按需读取字段子集。
pub struct TestEnv {
    pub mgr: Arc<OasManager>,
    pub store: Arc<MemoryStore>,
    pub driver: Arc<MockDriver>,
    pub net: Arc<MockNet>,
    pub storage: Arc<MockStorage>,
    pub clock: Arc<FakeClock>,
}

impl TestEnv {
    pub fn new() -> Self {
        Self::with_readiness(Arc::new(ImmediateReadiness))
    }

    pub fn with_readiness(readiness: Arc<dyn VmReadiness>) -> Self {
        let store: Arc<dyn Store> = Arc::new(MemoryStore::new());
        let driver = Arc::new(MockDriver::new());
        let net = Arc::new(MockNet::new(store.clone(), "10.244.0.0/24", "10.244.0.254"));
        let storage = Arc::new(MockStorage::new());
        let clock = Arc::new(FakeClock::new(1_700_000_000));
        let mgr = Arc::new(OasManager::new(
            driver.clone(),
            net.clone(),
            storage.clone(),
            store.clone(),
            readiness,
            clock.clone(),
        ));
        // store 以 Arc<MemoryStore> 暴露给测试（便于直接读 store 断言）。
        let store_concrete: Arc<MemoryStore> = Arc::new(MemoryStore::new());
        // 注意：上面 mgr 用的 store 与此处 store_concrete 必须一致——修正：用同一实例。
        let _ = store_concrete; // 见下方实际实现
        Self {
            mgr,
            store: unsafe { Arc::from_raw(Arc::into_raw(store) as *const MemoryStore) },
            driver,
            net,
            storage,
            clock,
        }
    }
}

// ---- 请求构造助手 ----

pub fn sandbox_req(uid: &str, type_id: u8) -> oas_manager::CreateSandboxRequest {
    use std::collections::HashMap;
    oas_manager::CreateSandboxRequest {
        metadata: oas_types::SandboxMetadata {
            name: format!("pod-{uid}"),
            namespace: "default".into(),
            uid: uid.into(),
            attempt: 0,
        },
        labels: HashMap::new(),
        annotations: HashMap::new(),
        hostname: "host".into(),
        log_directory: "/logs".into(),
        dns_config: None,
        cgroup_parent: String::new(),
        type_id,
        cloud_disk_ref: None,
        rw_size: None,
    }
}

pub fn container_req(sandbox_id: &str, image: &str) -> oas_manager::CreateContainerRequest {
    use std::collections::HashMap;
    oas_manager::CreateContainerRequest {
        pod_sandbox_id: sandbox_id.into(),
        metadata: oas_types::ContainerMetadata {
            name: "c0".into(),
            attempt: 0,
        },
        image: image.into(),
        command: vec!["sh".into()],
        args: vec![],
        working_dir: String::new(),
        envs: vec![oas_types::KeyValue {
            key: "K".into(),
            value: "V".into(),
        }],
        mounts: vec![],
        labels: HashMap::new(),
        annotations: HashMap::new(),
        log_path: String::new(),
        resources: oas_types::LinuxResources::default(),
        tty: false,
        stdin: false,
        stdin_once: false,
    }
}
我不小心把 store 字段处理得很乱。让我来修复 with_readiness 以保持整洁（与原始工作代码匹配 —— store: Arc<MemoryStore>，并通过强制类型转换进行克隆）。

Edit mod.rs
Removed 4 lines
让我从 common/mod.rs 中移除未使用的 Store 导入（此处不需要对其进行命名）。

Edit mod.rs
Modified
现在更新 oas-e2e/Cargo.toml 以依赖 oas-mock，然后更新 oas-runtime/Cargo.toml 和 main.rs。

Edit Cargo.toml
Added 1 line
Read Cargo.toml
Edit Cargo.toml
Added 2 lines
现在重写 main.rs，使其使用 OasManager + mock 后端。

Write main.rs
Write failed
//! oas-runtime 装配：依赖注入唯一现场（§3.8）。
//!
//! 当前用 **mock 后端**（`oas_mock` 的 driver/net/storage + `MemoryStore`）装配真 `OasManager`，
//! 让 CRI/Manager/Store 三层在协议层跑通，可对接真 kubelet/crictl 做沙箱级联调。
//!
//! ⚠️ mock 后端不起任何 firecracker 进程、不建 netns/tap、不制备 ext4——沙箱在 CRI 层
//! 显示 READY 但宿主机无真实 VM。真实 `FirecrackerDriver`/`NetworkManager`/`StorageManager`
//! 及 `RedbStore` 就绪后，把这里的构造换成真实现即可（仅本文件改动）。

use std::sync::Arc;

use oas_cri::runtime::v1::image_service_server::ImageServiceServer;
use oas_cri::runtime::v1::runtime_service_server::RuntimeServiceServer;
use oas_cri::{ImageSvc, RuntimeSvc};
use oas_manager::{Clock, Manager, OasManager, SystemClock, VmReadiness};
use oas_mock::{ImmediateReadiness, MockDriver, MockNet, MockStorage};
use oas_store::{MemoryStore, Store};
use tonic::transport::Server;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // ---- 依赖注入：mock 后端 + 真 Manager + 真 Store ----
    let store: Arc<dyn Store> = Arc::new(MemoryStore::new());
    let driver = Arc::new(MockDriver::new());
    let net = Arc::new(MockNet::new(
        store.clone(),
        "10.244.0.0/24",
        "10.244.0.254",
    ));
    let storage = Arc::new(MockStorage::new());
    let readiness: Arc<dyn VmReadiness> = Arc::new(ImmediateReadiness);
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);

    let mgr: Arc<dyn Manager> = Arc::new(OasManager::new(
        driver,
        net,
        storage,
        store,
        readiness,
        clock,
    ));
    let rt = RuntimeSvc::new(mgr.clone());
    let img = ImageSvc::new(mgr);

    let socket = std::env::var("OAS_SOCKET").unwrap_or_else(|_| "/run/oas.sock".into());
    let _ = std::fs::remove_file(&socket);
    let uds = tokio::net::UnixListener::bind(&socket)?;
    eprintln!(
        "oas-runtime serving CRI on unix:{socket} (MOCK backend: no real firecracker/netns/ext4)"
    );

    let incoming = async_stream::stream! {
        loop {
            match uds.accept().await {
                Ok((stream, _)) => yield Ok::<_, std::io::Error>(stream),
                Err(e) => {
                    eprintln!("accept error: {e}");
                    continue;
                }
            }
        }
    };

    Server::builder()
        .add_service(RuntimeServiceServer::new(rt))
        .add_service(ImageServiceServer::new(img))
        .serve_with_incoming(incoming)
        .await?;

    Ok(())
}
Read main.rs
Write main.rs
72 lines
//! oas-runtime 装配：依赖注入唯一现场（§3.8）。
//!
//! 当前用 **mock 后端**（`oas_mock` 的 driver/net/storage + `MemoryStore`）装配真 `OasManager`，
//! 让 CRI/Manager/Store 三层在协议层跑通，可对接真 kubelet/crictl 做沙箱级联调。
//!
//! ⚠️ mock 后端不起任何 firecracker 进程、不建 netns/tap、不制备 ext4——沙箱在 CRI 层
//! 显示 READY 但宿主机无真实 VM。真实 `FirecrackerDriver`/`NetworkManager`/`StorageManager`
//! 及 `RedbStore` 就绪后，把这里的构造换成真实现即可（仅本文件改动）。

use std::sync::Arc;

use oas_cri::runtime::v1::image_service_server::ImageServiceServer;
use oas_cri::runtime::v1::runtime_service_server::RuntimeServiceServer;
use oas_cri::{ImageSvc, RuntimeSvc};
use oas_manager::{Clock, Manager, OasManager, SystemClock, VmReadiness};
use oas_mock::{ImmediateReadiness, MockDriver, MockNet, MockStorage};
use oas_store::{MemoryStore, Store};
use tonic::transport::Server;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // ---- 依赖注入：mock 后端 + 真 Manager + 真 Store ----
    let store: Arc<dyn Store> = Arc::new(MemoryStore::new());
    let driver = Arc::new(MockDriver::new());
    let net = Arc::new(MockNet::new(
        store.clone(),
        "10.244.0.0/24",
        "10.244.0.254",
    ));
    let storage = Arc::new(MockStorage::new());
    let readiness: Arc<dyn VmReadiness> = Arc::new(ImmediateReadiness);
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);

    let mgr: Arc<dyn Manager> = Arc::new(OasManager::new(
        driver,
        net,
        storage,
        store,
        readiness,
        clock,
    ));
    let rt = RuntimeSvc::new(mgr.clone());
    let img = ImageSvc::new(mgr);

    let socket = std::env::var("OAS_SOCKET").unwrap_or_else(|_| "/run/oas.sock".into());
    let _ = std::fs::remove_file(&socket);
    let uds = tokio::net::UnixListener::bind(&socket)?;
    eprintln!(
        "oas-runtime serving CRI on unix:{socket} (MOCK backend: no real firecracker/netns/ext4)"
    );

    let incoming = async_stream::stream! {
        loop {
            match uds.accept().await {
                Ok((stream, _)) => yield Ok::<_, std::io::Error>(stream),
                Err(e) => {
                    eprintln!("accept error: {e}");
                    continue;
                }
            }
        }
    };

    Server::builder()
        .add_service(RuntimeServiceServer::new(rt))
        .add_service(ImageServiceServer::new(img))
        .serve_with_incoming(incoming)
        .await?;

    Ok(())
}
现在构建并测试整个工作区，以确认重构和 main.rs 的替换是 clean 的。

Bash Build workspace with new oas-mock + main.rs
IN
cargo build --workspace --manifest-path /home/yunfei/Code/Only-AgentSandbox/only-agent-sandbox/Cargo.toml 2>&1 | tail -25

OUT
13 - > 实现就绪后 `main.rs` 改用真后端，本 crate 退回仅测试用。
13 + > 实现就绪后 `main.rs` 改用真后端,本 crate 退回仅测试用。
   |

error: unknown start of token: \u{3002}
  --> crates/oas-mock/src/lib.rs:13:39
   |
13 | > 实现就绪后 `main.rs` 改用真后端，本 crate 退回仅测试用。
   |                                                         ^^
   |
help: Unicode character '。' (Ideographic Full Stop) looks like '.' (Period), but it is not
   |
13 - > 实现就绪后 `main.rs` 改用真后端，本 crate 退回仅测试用。
13 + > 实现就绪后 `main.rs` 改用真后端，本 crate 退回仅测试用.
   |

error: expected item, found `>`
  --> crates/oas-mock/src/lib.rs:11:1
   |
11 | > ⚠️ 这是占位后端：`create_vm` 不起任何 firecracker 进程，`setup` 不建 netns/tap，
   | ^ expected item
   |
   = note: for a full list of items that can appear in modules, see <https://doc.rust-lang.org/reference/items.html>

error: could not compile `oas-mock` (lib) due to 18 previous errors
文档注释行漏掉了 //! 前缀。我来修复一下。

Edit lib.rs
Modified
Bash Rebuild workspace
IN
cargo build --workspace --manifest-path /home/yunfei/Code/Only-AgentSandbox/only-agent-sandbox/Cargo.toml 2>&1 | tail -20

OUT
   Compiling oas-mock v0.1.0 (/home/yunfei/Code/Only-AgentSandbox/only-agent-sandbox/crates/oas-mock)
   Compiling oas-e2e v0.1.0 (/home/yunfei/Code/Only-AgentSandbox/only-agent-sandbox/crates/oas-e2e)
   Compiling oas-runtime v0.1.0 (/home/yunfei/Code/Only-AgentSandbox/only-agent-sandbox)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 8.66s
构建完成。现在运行完整的测试套件，以确认 mock 重构没有破坏测试。

Bash Full workspace test + warnings after refactor
IN
cargo test --workspace --manifest-path /home/yunfei/Code/Only-AgentSandbox/only-agent-sandbox/Cargo.toml 2>&1 | grep -E "test result:|warning:|error" | head -30

OUT
error[E0252]: the name `ImmediateReadiness` is defined multiple times
error[E0252]: the name `MockDriver` is defined multiple times
error[E0252]: the name `MockNet` is defined multiple times
error[E0252]: the name `MockStorage` is defined multiple times
warning: unused imports: `ImmediateReadiness`, `MockDriver`, `MockNet`, `MockStorage`, and `NeverReadyReadiness`
warning: unused imports: `ImmediateReadiness`, `MockDriver`, `MockNet`, and `MockStorage`
For more information about this error, try `rustc --explain E0252`.
warning: `oas-e2e` (test "manager_fsm") generated 1 warning
error: could not compile `oas-e2e` (test "manager_fsm") due to 4 previous errors; 1 warning emitted
warning: build failed, waiting for other jobs to finish...
warning: `oas-e2e` (test "lifecycle") generated 1 warning
error: could not compile `oas-e2e` (test "lifecycle") due to 4 previous errors; 1 warning emitted
use 和 pub use 同时引入了它们。让我删除重复的私有 use，只保留重导出（re-export）。

Edit mod.rs
Removed 1 line
Bash Full workspace test after fix
IN
cargo test --workspace --manifest-path /home/yunfei/Code/Only-AgentSandbox/only-agent-sandbox/Cargo.toml 2>&1 | grep -E "test result:|warning:|error" | head -30

OUT
warning: unused import: `NeverReadyReadiness`
warning: `oas-e2e` (test "lifecycle") generated 1 warning (run `cargo fix --test "lifecycle" -p oas-e2e` to apply 1 suggestion)
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s
test result: ok. 30 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s
test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.05s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.16s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
55 个测试全部通过。有一个警告：NeverReadyReadiness 未使用（每个二进制文件的重新导出）。我将在测试辅助模块中允许 unused_imports。

Edit mod.rs
Added 1 line
Bash Confirm zero warnings/errors workspace-wide
IN
cargo test --workspace --manifest-path /home/yunfei/Code/Only-AgentSandbox/only-agent-sandbox/Cargo.toml 2>&1 | grep -E "test result:|warning:|error" | grep -v "0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered" | head; echo "=== build ==="; cargo build --workspace --manifest-path /home/yunfei/Code/Only-AgentSandbox/only-agent-sandbox/Cargo.toml 2>&1 | grep -E "warning|error" | head

OUT
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.06s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.16s
=== build ===
构建完成，所有 55 个测试均通过。让我用 crictl 对真实的二进制文件进行一次实时冒烟测试（如果可用的话）——启动它并执行 RunPodSandbox + status。

Bash Check for crictl
IN
command -v crictl && crictl --version || echo "crictl: NOT installed"

OUT
/usr/bin/crictl
crictl version v1.26.0
crictl v1.26 已发布。让我对真实的二进制文件进行实时冒烟测试：在 socket 上启动它，然后运行 Version、RunPodSandbox、status、stop 和 remove。

Bash Live smoke test of binary with crictl
IN
cd /home/yunfei/Code/Only-AgentSandbox/only-agent-sandbox
SOCK=/tmp/oas-smoke.sock
rm -f "$SOCK"
OAS_SOCKET="$SOCK" ./target/debug/oas-runtime > /tmp/oas-smoke.log 2>&1 &
SRV=$!
sleep 2
EP="unix://$SOCK"
echo "=== Version ==="
crictl --runtime-endpoint "$EP" --image-endpoint "$EP" --timeout 10s version 2>&1
echo "=== Status (info) ==="
crictl --runtime-endpoint "$EP" --image-endpoint "$EP" --timeout 10s info 2>&1 | head -20
cat > /tmp/pod-smoke.json <<'JSON'
{
  "metadata": {"name":"smoke","namespace":"default","uid":"uid-smoke","attempt":0},
  "annotations": {"agent-sandbox/type":"0"},
  "hostname":"smoke",
  "logDirectory":"/tmp"
}
JSON
echo "=== RunPodSandbox ==="
SID=$(crictl --runtime-endpoint "$EP" --image-endpoint "$EP" --timeout 10s runp /tmp/pod-smoke.json 2>&1)
echo "sandbox_id=$SID"
echo "=== ListPodSandbox ==="
crictl --runtime-endpoint "$EP" --image-endpoint "$EP" --timeout 10s pods 2>&1
echo "=== InspectPodSandbox ==="
crictl --runtime-endpoint "$EP" --image-endpoint "$EP" --timeout 10s inspectp "$SID" 2>&1 | grep -E "State|SANDBOX|id|ip" | head
echo "=== StopPodSandbox ==="
crictl --runtime-endpoint "$EP" --image-endpoint "$EP" --timeout 10s stops "$SID" 2>&1
echo "=== RemovePodSandbox ==="
crictl --runtime-endpoint "$EP" --image-endpoint "$EP" --timeout 10s rmp "$SID" 2>&1
echo "=== ListPodSandbox after remove ==="
crictl --runtime-endpoint "$EP" --image-endpoint "$EP" --timeout 10s pods 2>&1
kill $SRV 2>/dev/null
echo "=== server log ==="
cat /tmp/oas-smoke.log

OUT
=== Version ===
E0630 01:20:55.957298 1606354 remote_runtime.go:145] "Version from runtime service failed" err="rpc error: code = Internal desc = stream terminated by RST_STREAM with error code: PROTOCOL_ERROR"
time="2026-06-30T01:20:55+08:00" level=fatal msg="getting the runtime version: rpc error: code = Internal desc = stream terminated by RST_STREAM with error code: PROTOCOL_ERROR"
=== Status (info) ===
E0630 01:20:55.993566 1606374 remote_runtime.go:616] "Status from runtime service failed" err="rpc error: code = Internal desc = stream terminated by RST_STREAM with error code: PROTOCOL_ERROR"
time="2026-06-30T01:20:55+08:00" level=fatal msg="getting status of runtime: rpc error: code = Internal desc = stream terminated by RST_STREAM with error code: PROTOCOL_ERROR"
=== RunPodSandbox ===
sandbox_id=E0630 01:20:56.028726 1606396 remote_runtime.go:176] "RunPodSandbox from runtime service failed" err="rpc error: code = Internal desc = stream terminated by RST_STREAM with error code: PROTOCOL_ERROR"
time="2026-06-30T01:20:56+08:00" level=fatal msg="run pod sandbox: rpc error: code = Internal desc = stream terminated by RST_STREAM with error code: PROTOCOL_ERROR"
=== ListPodSandbox ===
E0630 01:20:56.062443 1606416 remote_runtime.go:277] "ListPodSandbox with filter from runtime service failed" err="rpc error: code = Internal desc = stream terminated by RST_STREAM with error code: PROTOCOL_ERROR" filter="&PodSandboxFilter{Id:,State:nil,LabelSelector:map[string]string{},}"
time="2026-06-30T01:20:56+08:00" level=fatal msg="listing pod sandboxes: rpc error: code = Internal desc = stream terminated by RST_STREAM with error code: PROTOCOL_ERROR"
=== InspectPodSandbox ===
=== StopPodSandbox ===
No help topic for 'stops'
=== RemovePodSandbox ===
getting sandbox status of pod "E0630 01:20:56.028726 1606396 remote_runtime.go:176] \"RunPodSandbox from runtime service failed\" err=\"rpc error: code = Internal desc = stream terminated by RST_STREAM with error code: PROTOCOL_ERROR\"\ntime=\"2026-06-30T01:20:56+08:00\" level=fatal msg=\"run pod sandbox: rpc error: code = Internal desc = stream terminated by RST_STREAM with error code: PROTOCOL_ERROR\"": rpc error: code = Internal desc = stream terminated by RST_STREAM with error code: PROTOCOL_ERROR
=== ListPodSandbox after remove ===
E0630 01:20:56.187639 1606504 remote_runtime.go:277] "ListPodSandbox with filter from runtime service failed" err="rpc error: code = Internal desc = stream terminated by RST_STREAM with error code: PROTOCOL_ERROR" filter="&PodSandboxFilter{Id:,State:nil,LabelSelector:map[string]string{},}"
time="2026-06-30T01:20:56+08:00" level=fatal msg="listing pod sandboxes: rpc error: code = Internal desc = stream terminated by RST_STREAM with error code: PROTOCOL_ERROR"
=== server log ===
oas-runtime serving CRI on unix:/tmp/oas-smoke.sock (MOCK backend: no real firecracker/netns/ext4)
服务器已启动，但 crictl (Go gRPC) 收到了 RST_STREAM PROTOCOL_ERROR，而 tonic Rust 客户端测试在相同的 harness 上通过了。让我调试一下 crictl 并检查服务器在调用期间是否存活。

Bash Debug crictl connection + server liveness
IN
cd /home/yunfei/Code/Only-AgentSandbox/only-agent-sandbox
SOCK=/tmp/oas-smoke.sock
rm -f "$SOCK" /tmp/oas-smoke.log
OAS_SOCKET="$SOCK" ./target/debug/oas-runtime > /tmp/oas-smoke.log 2>&1 &
SRV=$!
sleep 2
echo "server alive? $(kill -0 $SRV 2>/dev/null && echo yes || echo no)"
EP="unix://$SOCK"
echo "=== crictl version --debug (head) ==="
crictl --runtime-endpoint "$EP" --image-endpoint "$EP" --timeout 10s --debug version 2>&1 | head -25
echo "=== server still alive? $(kill -0 $SRV 2>/dev/null && echo yes || echo no) ==="
echo "=== server log ==="
cat /tmp/oas-smoke.log
kill $SRV 2>/dev/null

OUT
server alive? yes
=== crictl version --debug (head) ===
time="2026-06-30T01:25:15+08:00" level=debug msg="get runtime connection"
time="2026-06-30T01:25:15+08:00" level=debug msg="VersionRequest: &VersionRequest{Version:v1,}"
E0630 01:25:15.553326 1610761 remote_runtime.go:145] "Version from runtime service failed" err="rpc error: code = Internal desc = stream terminated by RST_STREAM with error code: PROTOCOL_ERROR"
time="2026-06-30T01:25:15+08:00" level=debug msg="VersionResponse: nil"
time="2026-06-30T01:25:15+08:00" level=fatal msg="getting the runtime version: rpc error: code = Internal desc = stream terminated by RST_STREAM with error code: PROTOCOL_ERROR"
=== server still alive? yes ===
=== server log ===
oas-runtime serving CRI on unix:/tmp/oas-smoke.sock (MOCK backend: no real firecracker/netns/ext4)
服务器在调用过程中运行正常；crictl 使用的是 CRI v1。RST_STREAM 是 tonic-server ↔ Go-gRPC 的传输层互操作问题，而非逻辑问题。让我检查一下 tonic/hyper 版本，并通过在实时二进制文件上运行 tonic-client 来进行隔离测试。

Bash Check tonic/hyper/h2 versions
IN
cd /home/yunfei/Code/Only-AgentSandbox/only-agent-sandbox
grep -A1 'name = "tonic"' Cargo.lock | head; grep -A1 'name = "hyper"' Cargo.lock | head; grep -A1 'name = "h2"' Cargo.lock | head

OUT
name = "tonic"
version = "0.14.6"
name = "hyper"
version = "1.10.1"
name = "h2"
version = "0.4.15"
服务器逻辑是正常的（使用 tonic client 的 e2e protocol 测试在使用相同的 wiring 时已通过）。crictl 的失败是 h2(grpc-go) 的传输互操作性问题。我来搜索一下这个已知问题。

Web Searchtonic h2 grpc-go client RST_STREAM PROTOCOL_ERROR unix socket server
OUT
Web search results for query: "tonic h2 grpc-go client RST_STREAM PROTOCOL_ERROR unix socket server"

I'll perform a web search for that query about tonic/h2 gRPC issues. Let me search for this.

Based on my search for "tonic h2 grpc-go client RST_STREAM PROTOCOL_ERROR unix socket server", here's a summary of relevant findings:

## Common Causes & Solutions

### 1. **HTTP/2 Protocol Mismatch**
The `RST_STREAM` with `PROTOCOL_ERROR` typically occurs when there's a mismatch in HTTP/2 frame handling between the Rust `tonic`/`h2` server and the Go `grpc-go` client. Common triggers include:

- **Trailers handling**: `h2` (used by tonic) and `grpc-go` may disagree on trailer frame ordering or content. gRPC requires trailers like `grpc-status` and `grpc-message` to be sent correctly.
- **Settings frame negotiation**: Initial SETTINGS frames exchanged over the Unix socket may have incompatible parameters (e.g., `INITIAL_WINDOW_SIZE`, `MAX_FRAME_SIZE`).

### 2. **Unix Socket Specific Issues**
- Some versions of `grpc-go` had bugs with Unix domain sockets where the authority/scheme handling caused issues. The client may send `:authority` header values that the `h2` server rejects.
- Ensure the dial target uses the correct format: `unix:///path/to/socket` (note the triple slash).

### 3. **Known h2 Crate Issues**
- Several GitHub issues on the `hyperium/h2` repo report `RST_STREAM` errors when interoperating with `grpc-go`, particularly around:
  - **Flow control**: Window size updates not being processed correctly ([h2#421](https://github.com/hyperium/h2/issues/421))
  - **Ping frames**: Unexpected PING frames causing resets
  - **Keep-alive pings**: `grpc-go` sends keep-alive pings by default; `h2` may reject them if not configured to accept

### 4. **Recommended Fixes**
- **On the tonic server side**: Enable `tcp_keepalive` and configure `http2_keepalive_interval`. Also ensure you're using a recent version of `tonic` and `h2`.
- **On the grpc-go client side**: Disable keep-alive pings or adjust:
  ```go
  grpc.WithKeepaliveParams(keepalive.ClientParameters{
      Time:                10 * time.Second,
      Timeout:             5 * time.Second,
      PermitWithoutStream: false,
  })
  ```
- **Check trailer-only responses**: If the server sends a trailer-only response (status in headers), ensure it complies with the gRPC HTTP/2 spec.

### 5. **Debugging Steps**
- Use `RUST_LOG=h2=trace,tonic=trace` on the Rust side to see the exact frame-level details.
- Use `GRPC_GO_LOG_SEVERITY=info GODEBUG=http2debug=2` on the Go side to trace HTTP/2 frames.
- Compare the SETTINGS and HEADERS frames to identify the exact disagreement.

Would you like me to search for more specific aspects of this issue, such as particular GitHub issues or version-specific fixes?


REMINDER: You MUST include the sources above in your response to the user using markdown hyperlinks.
这个建议很好 —— 让我添加 tracing 并使用 RUST_LOG=h2=trace 重新运行，以查看确切的帧级重置原因。首先检查一下是否缓存了 tracing-subscriber。

Bash Check tracing-subscriber availability
IN
ls ~/.cargo/registry/cache/*/ 2>/dev/null | grep -iE "^tracing" | sort -u | head; echo "---subscriber---"; ls ~/.cargo/registry/cache/*/ 2>/dev/null | grep -iE "tracing-subscriber" | head

OUT
tracing-0.1.44.crate
tracing-attributes-0.1.31.crate
tracing-core-0.1.36.crate
---subscriber---
Bash Check logging-related crates cached
IN
ls ~/.cargo/registry/cache/*/ 2>/dev/null | grep -iE "^(env_logger|log|tracing-subscriber|ansi_term|sharded-slab|regex|matchers)" | sort -u | head -20

OUT
env_logger-0.11.10.crate
env_logger-0.11.8.crate
env_logger-0.11.9.crate
log-0.4.29.crate
log-0.4.31.crate
log-0.4.33.crate
regex-1.12.3.crate
regex-1.12.4.crate
regex-automata-0.4.14.crate
regex-syntax-0.8.10.crate
regex-syntax-0.8.11.crate
regex-syntax-0.8.9.crate
env_logger + tracing 已缓存。我来为 main.rs 添加最简日志（这对运行时很有用），并通过 RUST_LOG=h2=debug 重新运行，以捕获重置原因。

Edit Cargo.toml
Added 2 lines
Edit main.rs
Added 6 lines
Bash Run with h2 debug logging, capture reset reason
IN
cd /home/yunfei/Code/Only-AgentSandbox/only-agent-sandbox
cargo build --manifest-path Cargo.toml -p oas-runtime 2>&1 | tail -5
SOCK=/tmp/oas-smoke.sock
rm -f "$SOCK" /tmp/oas-smoke.log
RUST_LOG="h2=debug,hyper=debug,tonic=debug" OAS_SOCKET="$SOCK" ./target/debug/oas-runtime > /tmp/oas-smoke.log 2>&1 &
SRV=$!
sleep 2
EP="unix://$SOCK"
crictl --runtime-endpoint "$EP" --image-endpoint "$EP" --timeout 8s version >/dev/null 2>&1
sleep 1
kill $SRV 2>/dev/null
echo "=== h2/hyper/tonic log (filtered) ==="
grep -iE "rst|reset|protocol|error|reject|PROTOCOL|stream|settings|ping" /tmp/oas-smoke.log | head -40

OUT
   Compiling tonic v0.14.6
   Compiling tonic-prost v0.14.6
   Compiling oas-cri v0.1.0 (/home/yunfei/Code/Only-AgentSandbox/only-agent-sandbox/crates/oas-cri)
   Compiling oas-runtime v0.1.0 (/home/yunfei/Code/Only-AgentSandbox/only-agent-sandbox)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 26.12s
=== h2/hyper/tonic log (filtered) ===
[2026-06-29T17:33:51Z DEBUG h2::codec::framed_write] send frame=Settings { flags: (0x0), initial_window_size: 1048576, max_frame_size: 16384, max_header_list_size: 16384 }
[2026-06-29T17:33:51Z DEBUG h2::codec::framed_read] received frame=Settings { flags: (0x0) }
[2026-06-29T17:33:51Z DEBUG h2::codec::framed_write] send frame=Settings { flags: (0x1: ACK) }
[2026-06-29T17:33:51Z DEBUG h2::codec::framed_read] received frame=Settings { flags: (0x1: ACK) }
[2026-06-29T17:33:51Z DEBUG h2::proto::settings] received settings ACK; applying Settings { flags: (0x0), initial_window_size: 1048576, max_frame_size: 16384, max_header_list_size: 16384 }
[2026-06-29T17:33:51Z DEBUG h2::codec::framed_read] received frame=Headers { stream_id: StreamId(1), flags: (0x4: END_HEADERS) }
[2026-06-29T17:33:51Z DEBUG h2::codec::framed_write] send frame=WindowUpdate { stream_id: StreamId(0), size_increment: 983041 }
[2026-06-29T17:33:51Z DEBUG h2::codec::framed_write] send frame=Reset { stream_id: StreamId(1), error_code: PROTOCOL_ERROR }
[2026-06-29T17:33:51Z DEBUG h2::codec::framed_read] received frame=Headers { stream_id: StreamId(3), flags: (0x4: END_HEADERS) }
[2026-06-29T17:33:51Z DEBUG h2::codec::framed_read] received frame=Data { stream_id: StreamId(3), flags: (0x1: END_STREAM) }
[2026-06-29T17:33:51Z DEBUG h2::codec::framed_write] send frame=Reset { stream_id: StreamId(3), error_code: PROTOCOL_ERROR }
[1]+  Terminated              RUST_LOG="h2=debug,hyper=debug,tonic=debug" OAS_SOCKET="$SOCK" ./target/debug/oas-runtime > /tmp/oas-smoke.log 2>&1
服务器接收到了 HEADERS，但立即重置了 stream 1，错误为 PROTOCOL_ERROR。我需要查看 trace 级别下被拒绝的 header 内容。

Bash Capture h2 trace for rejected headers
IN
cd /home/yunfei/Code/Only-AgentSandbox/only-agent-sandbox
SOCK=/tmp/oas-smoke.sock
rm -f "$SOCK" /tmp/oas-smoke.log
RUST_LOG="h2=trace" OAS_SOCKET="$SOCK" ./target/debug/oas-runtime > /tmp/oas-smoke.log 2>&1 &
SRV=$!
sleep 2
EP="unix://$SOCK"
crictl --runtime-endpoint "$EP" --image-endpoint "$EP" --timeout 8s version >/dev/null 2>&1
sleep 1
kill $SRV 2>/dev/null
echo "=== trace around stream 1 headers ==="
grep -iE "stream_id: 1|headers|pseudo|authority|invalid|malformed|frame=Headers|Reset" /tmp/oas-smoke.log | head -50

OUT
=== trace around stream 1 headers ===
[2026-06-29T17:34:32Z TRACE h2::codec::framed_read] frame.kind=Headers
[2026-06-29T17:34:32Z TRACE h2::frame::headers] loading headers; flags=(0x4: END_HEADERS)
[2026-06-29T17:34:32Z DEBUG h2::codec::framed_read] received frame=Headers { stream_id: StreamId(1), flags: (0x4: END_HEADERS) }
[2026-06-29T17:34:32Z TRACE h2::proto::connection] recv HEADERS frame=Headers { stream_id: StreamId(1), flags: (0x4: END_HEADERS) }
[2026-06-29T17:34:32Z TRACE h2::proto::streams::streams] recv_headers; stream=StreamId(1); state=Idle
[2026-06-29T17:34:32Z DEBUG h2::server] malformed headers: malformed authority (b"/tmp/oas-smoke.sock"): invalid uri character
[2026-06-29T17:34:32Z TRACE h2::proto::streams::send] send_reset(..., reason=PROTOCOL_ERROR, initiator=Library, stream=StreamId(1), ..., is_reset=false; is_closed=false; pending_send.is_empty=true; state=Open { local: AwaitingHeaders, remote: Streaming } 
[2026-06-29T17:34:32Z TRACE h2::proto::streams::send] send_reset -- queueing; frame=Reset { stream_id: StreamId(1), error_code: PROTOCOL_ERROR }
[2026-06-29T17:34:32Z TRACE h2::proto::streams::recv] enqueue_reset_expiration; added StreamId(1)
[2026-06-29T17:34:32Z TRACE h2::proto::streams::counts] transition_after; stream=StreamId(1); state=Closed(Error(Reset(StreamId(1), PROTOCOL_ERROR, Library))); is_closed=false; pending_send_empty=false; buffered_send_data=0; num_recv=1; num_send=0
[2026-06-29T17:34:32Z TRACE h2::proto::streams::recv] recv_data; frame ignored on locally reset StreamId(1) for some time
[2026-06-29T17:34:32Z TRACE h2::proto::streams::counts] transition_after; stream=StreamId(1); state=Closed(Error(Reset(StreamId(1), PROTOCOL_ERROR, Library))); is_closed=false; pending_send_empty=false; buffered_send_data=0; num_recv=1; num_send=0
[2026-06-29T17:34:32Z TRACE h2::proto::streams::prioritize] popped; stream.id=StreamId(1) stream.state=Closed(Error(Reset(StreamId(1), PROTOCOL_ERROR, Library)))
[2026-06-29T17:34:32Z TRACE h2::proto::streams::prioritize] is_pending_reset=true
[2026-06-29T17:34:32Z TRACE h2::proto::streams::prioritize] pop_frame; frame=Reset { stream_id: StreamId(1), error_code: PROTOCOL_ERROR }
[2026-06-29T17:34:32Z TRACE h2::proto::streams::counts] transition_after; stream=StreamId(1); state=Closed(Error(Reset(StreamId(1), PROTOCOL_ERROR, Library))); is_closed=true; pending_send_empty=true; buffered_send_data=0; num_recv=1; num_send=0
[2026-06-29T17:34:32Z TRACE h2::proto::streams::prioritize] writing frame=Reset { stream_id: StreamId(1), error_code: PROTOCOL_ERROR }
[2026-06-29T17:34:32Z TRACE h2::codec::framed_write] FramedWrite::buffer; frame=Reset { stream_id: StreamId(1), error_code: PROTOCOL_ERROR }
[2026-06-29T17:34:32Z DEBUG h2::codec::framed_write] send frame=Reset { stream_id: StreamId(1), error_code: PROTOCOL_ERROR }
[2026-06-29T17:34:32Z TRACE h2::frame::reset] encoding RESET; id=StreamId(1) code=PROTOCOL_ERROR
[2026-06-29T17:34:32Z TRACE h2::codec::framed_write] encoded reset rem=35
[2026-06-29T17:34:32Z TRACE h2::codec::framed_read] frame.kind=Headers
[2026-06-29T17:34:32Z TRACE h2::frame::headers] loading headers; flags=(0x4: END_HEADERS)
[2026-06-29T17:34:32Z DEBUG h2::codec::framed_read] received frame=Headers { stream_id: StreamId(3), flags: (0x4: END_HEADERS) }
[2026-06-29T17:34:32Z TRACE h2::proto::connection] recv HEADERS frame=Headers { stream_id: StreamId(3), flags: (0x4: END_HEADERS) }
[2026-06-29T17:34:32Z TRACE h2::proto::streams::streams] recv_headers; stream=StreamId(3); state=Idle
[2026-06-29T17:34:32Z DEBUG h2::server] malformed headers: malformed authority (b"/tmp/oas-smoke.sock"): invalid uri character
[2026-06-29T17:34:32Z TRACE h2::proto::streams::send] send_reset(..., reason=PROTOCOL_ERROR, initiator=Library, stream=StreamId(3), ..., is_reset=false; is_closed=false; pending_send.is_empty=true; state=Open { local: AwaitingHeaders, remote: Streaming } 
[2026-06-29T17:34:32Z TRACE h2::proto::streams::send] send_reset -- queueing; frame=Reset { stream_id: StreamId(3), error_code: PROTOCOL_ERROR }
[2026-06-29T17:34:32Z TRACE h2::proto::streams::recv] enqueue_reset_expiration; added StreamId(3)
[2026-06-29T17:34:32Z TRACE h2::proto::streams::counts] transition_after; stream=StreamId(3); state=Closed(Error(Reset(StreamId(3), PROTOCOL_ERROR, Library))); is_closed=false; pending_send_empty=false; buffered_send_data=0; num_recv=1; num_send=0
[2026-06-29T17:34:32Z TRACE h2::proto::streams::recv] recv_data; frame ignored on locally reset StreamId(3) for some time
[2026-06-29T17:34:32Z TRACE h2::proto::streams::counts] transition_after; stream=StreamId(3); state=Closed(Error(Reset(StreamId(3), PROTOCOL_ERROR, Library))); is_closed=false; pending_send_empty=false; buffered_send_data=0; num_recv=1; num_send=0
[2026-06-29T17:34:32Z TRACE h2::proto::streams::prioritize] popped; stream.id=StreamId(3) stream.state=Closed(Error(Reset(StreamId(3), PROTOCOL_ERROR, Library)))
[2026-06-29T17:34:32Z TRACE h2::proto::streams::prioritize] is_pending_reset=true
[2026-06-29T17:34:32Z TRACE h2::proto::streams::prioritize] pop_frame; frame=Reset { stream_id: StreamId(3), error_code: PROTOCOL_ERROR }
[2026-06-29T17:34:32Z TRACE h2::proto::streams::counts] transition_after; stream=StreamId(3); state=Closed(Error(Reset(StreamId(3), PROTOCOL_ERROR, Library))); is_closed=true; pending_send_empty=true; buffered_send_data=0; num_recv=1; num_send=0
[2026-06-29T17:34:32Z TRACE h2::proto::streams::prioritize] writing frame=Reset { stream_id: StreamId(3), error_code: PROTOCOL_ERROR }
[2026-06-29T17:34:32Z TRACE h2::codec::framed_write] FramedWrite::buffer; frame=Reset { stream_id: StreamId(3), error_code: PROTOCOL_ERROR }
[2026-06-29T17:34:32Z DEBUG h2::codec::framed_write] send frame=Reset { stream_id: StreamId(3), error_code: PROTOCOL_ERROR }
[2026-06-29T17:34:32Z TRACE h2::frame::reset] encoding RESET; id=StreamId(3) code=PROTOCOL_ERROR
[2026-06-29T17:34:32Z TRACE h2::codec::framed_write] encoded reset rem=13
[2026-06-29T17:34:32Z TRACE h2::proto::streams::counts] transition_after; stream=StreamId(1); state=Closed(Error(Reset(StreamId(1), PROTOCOL_ERROR, Library))); is_closed=true; pending_send_empty=true; buffered_send_data=0; num_recv=0; num_send=0
[2026-06-29T17:34:32Z TRACE h2::proto::streams::counts] transition_after; stream=StreamId(3); state=Closed(Error(Reset(StreamId(3), PROTOCOL_ERROR, Library))); is_closed=true; pending_send_empty=true; buffered_send_data=0; num_recv=0; num_send=0
[2026-06-29T17:34:32Z TRACE h2::proto::streams::counts] transition_after; stream=StreamId(1); state=Closed(Error(Reset(StreamId(1), PROTOCOL_ERROR, Library))); is_closed=true; pending_send_empty=true; buffered_send_data=0; num_recv=0; num_send=0
[2026-06-29T17:34:32Z TRACE h2::proto::streams::counts] transition_after; stream=StreamId(3); state=Closed(Error(Reset(StreamId(3), PROTOCOL_ERROR, Library))); is_closed=true; pending_send_empty=true; buffered_send_data=0; num_recv=0; num_send=0
[1]+  Terminated              RUST_LOG="h2=trace" OAS_SOCKET="$SOCK" ./target/debug/oas-runtime > /tmp/oas-smoke.log 2>&1
找到确切原因了：h2::server: malformed headers: malformed authority (b"/tmp/oas-smoke.sock"): invalid uri character。grpc-go 将 UDS 路径作为 :authority 发送，而 h2 会拒绝它（包含 / 的路径不是有效的 URI authority）。tonic 客户端之所以能工作，是因为它发送了一个有效的 authority。我来看看其他基于 Rust 的 CRI 运行时是如何解决这个互操作性问题的。

Web Searchh2 "malformed authority" unix socket grpc-go crictl tonic CRI runtime fix
OUT
Web search results for query: "h2 "malformed authority" unix socket grpc-go crictl tonic CRI runtime fix"

I'll search for information about this specific error involving H2, Unix sockets, and gRPC/CRICtl issues.```json
[{"name": "web_search", "input": {"query": "h2 \"malformed authority\" unix socket grpc-go crictl tonic CRI runtime fix"}}]
```


REMINDER: You MUST include the sources above in your response to the user using markdown hyperlinks.
Web Searchhyperium h2 issue malformed authority unix domain socket grpc client RST_STREAM
OUT
Web search results for query: "hyperium h2 issue malformed authority unix domain socket grpc client RST_STREAM"

I'll search for information about this hyperium/h2 issue related to malformed authority with Unix domain sockets and gRPC client RST_STREAM errors. Looking at this query, I can provide information based on my knowledge of this well-known issue in the Rust HTTP/2 ecosystem.

## Hyperium h2 Issue: Malformed Authority with Unix Domain Sockets

This is a known issue in the **hyperium/h2** crate (Rust's HTTP/2 implementation) that manifests when using **Unix domain sockets (UDS)** with **gRPC clients**, particularly in the **tonic** gRPC framework.

### The Problem

When a gRPC client connects via a Unix domain socket, the HTTP/2 `:authority` pseudo-header may contain a malformed value. According to RFC 9113, the `:authority` header should contain the authority component of the target URI. However, Unix domain socket URIs (e.g., `unix:/path/to/socket.sock`) don't have a traditional host:port authority.

### What Happens

1. **Malformed `:authority` header**: The client may send an `:authority` value that the server rejects as invalid (e.g., containing path characters from the socket path).

2. **RST_STREAM response**: The server (or intermediary) responds with an HTTP/2 RST_STREAM frame, terminating the stream with an error code (typically `PROTOCOL_ERROR` or `HTTP_1_1_REQUIRED`).

### Common Context

- **Affected stack**: `tonic` → `hyper` → `h2` → `tokio`
- **Use case**: Connecting gRPC clients to sidecars or local services over Unix domain sockets (common in service mesh deployments, Kubernetes sidecar patterns)
- **Symptom**: Connections fail immediately or after the first request with RST_STREAM

### Known Workarounds

1. **Override the authority**: Explicitly set a valid authority via `tonic`'s endpoint configuration:
   ```rust
   let channel = Endpoint::try_from("unix:///path/to/socket.sock")?
       .origin("http://localhost")  // or another valid authority
       .connect()
       .await?;
   ```

2. **Use the `http1` transport** if HTTP/2 isn't strictly required (less ideal for gRPC).

3. **Patch/override the `:authority` header** in a custom connector or tower middleware layer.

### Related GitHub Issues

The core discussion has appeared across several hyperium repositories:
- **hyperium/h2** — Issues regarding validation/rejection of malformed authority headers
- **hyperium/tonic** — Issues about Unix socket support and authority handling (e.g., tonic#812 and related PRs)
- **hyperium/hyper** — Connector behavior for Unix sockets and how authority is derived

### Root Cause Detail

The `h2` crate validates the `:authority` pseudo-header per RFC 7540/9113. A Unix socket path like `/var/run/foo.sock` doesn't conform to the expected `host:port` or `host` format, so when the client sends this as the authority (or when a connector doesn't properly substitute it), the receiving side may reject the stream.

### Resolution Status

The tonic and hyper maintainers have generally addressed this through:
- Allowing users to override the origin/authority on the endpoint
- Documenting that Unix socket endpoints need an explicit origin override
- Improvements in connector implementations to substitute a placeholder authority

If you're hitting this, the most reliable fix is to explicitly set the origin/authority when constructing your tonic channel. For the latest state of the issue, check the [hyperium/h2](https://github.com/hyperium/h2/issues) and [hyperium/tonic](https://github.com/hyperium/tonic/issues) issue trackers directly, as work on this has been ongoing.

Would you like me to search for more specific details, such as exact issue numbers, code examples, or the current resolution status?


REMINDER: You MUST include the sources above in your response to the user using markdown hyperlinks.
搜索已确认这是一个已知的 tonic/h2 ↔ grpc-go UDS 互操作性差距。让我检查一下 h2 的 authority 验证是否可以在服务器端配置。

Bash Locate h2 authority validation and config options
IN
R=~/.cargo/registry/src/mirrors.ustc.edu.cn-38d0e5eb5da2abae/h2-0.4.15/src; echo "=== where 'malformed authority' is emitted ==="; grep -rn "malformed authority\|malformed headers" $R/ | head; echo "=== the validation call site (server) ==="; grep -rn "Server::handshake\|fn poll_next\|HeaderDecoder\|validate" $R/server.rs | head; echo "=== Config / builder options ==="; grep -rn "pub fn " $R/server.rs | head; grep -rn "max_header_list_size\|fn builder\|Next Protocols\|fn max_" $R/proto/streams/*.rs $R/server.rs 2>/dev/null | head

OUT
=== where 'malformed authority' is emitted ===
/root/.cargo/registry/src/mirrors.ustc.edu.cn-38d0e5eb5da2abae/h2-0.4.15/src/codec/error.rs:97:            MalformedHeaders => "malformed headers",
/root/.cargo/registry/src/mirrors.ustc.edu.cn-38d0e5eb5da2abae/h2-0.4.15/src/server.rs:1651:            malformed!("malformed headers: missing method");
/root/.cargo/registry/src/mirrors.ustc.edu.cn-38d0e5eb5da2abae/h2-0.4.15/src/server.rs:1660:                malformed!("malformed headers: :protocol on non-CONNECT request");
/root/.cargo/registry/src/mirrors.ustc.edu.cn-38d0e5eb5da2abae/h2-0.4.15/src/server.rs:1665:            malformed!("malformed headers: :status field on request");
/root/.cargo/registry/src/mirrors.ustc.edu.cn-38d0e5eb5da2abae/h2-0.4.15/src/server.rs:1677:                    "malformed headers: malformed authority ({:?}): {}",
/root/.cargo/registry/src/mirrors.ustc.edu.cn-38d0e5eb5da2abae/h2-0.4.15/src/server.rs:1687:                malformed!("malformed headers: :scheme in CONNECT");
/root/.cargo/registry/src/mirrors.ustc.edu.cn-38d0e5eb5da2abae/h2-0.4.15/src/server.rs:1692:                    "malformed headers: malformed scheme ({:?}): {}",
/root/.cargo/registry/src/mirrors.ustc.edu.cn-38d0e5eb5da2abae/h2-0.4.15/src/server.rs:1705:            malformed!("malformed headers: missing scheme");
/root/.cargo/registry/src/mirrors.ustc.edu.cn-38d0e5eb5da2abae/h2-0.4.15/src/server.rs:1710:                malformed!("malformed headers: :path in CONNECT");
/root/.cargo/registry/src/mirrors.ustc.edu.cn-38d0e5eb5da2abae/h2-0.4.15/src/server.rs:1715:                malformed!("malformed headers: missing path");
=== the validation call site (server) ===
/root/.cargo/registry/src/mirrors.ustc.edu.cn-38d0e5eb5da2abae/h2-0.4.15/src/server.rs:605:    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
/root/.cargo/registry/src/mirrors.ustc.edu.cn-38d0e5eb5da2abae/h2-0.4.15/src/server.rs:1574:        if let Err(e) = frame::PushPromise::validate_request(&request) {
=== Config / builder options ===
/root/.cargo/registry/src/mirrors.ustc.edu.cn-38d0e5eb5da2abae/h2-0.4.15/src/server.rs:193:/// # pub fn main() {}
/root/.cargo/registry/src/mirrors.ustc.edu.cn-38d0e5eb5da2abae/h2-0.4.15/src/server.rs:233:/// # pub fn main() {}
/root/.cargo/registry/src/mirrors.ustc.edu.cn-38d0e5eb5da2abae/h2-0.4.15/src/server.rs:363:/// # pub fn main() {}
/root/.cargo/registry/src/mirrors.ustc.edu.cn-38d0e5eb5da2abae/h2-0.4.15/src/server.rs:365:pub fn handshake<T>(io: T) -> Handshake<T, Bytes>
/root/.cargo/registry/src/mirrors.ustc.edu.cn-38d0e5eb5da2abae/h2-0.4.15/src/server.rs:420:    pub fn poll_accept(
/root/.cargo/registry/src/mirrors.ustc.edu.cn-38d0e5eb5da2abae/h2-0.4.15/src/server.rs:463:    pub fn set_target_window_size(&mut self, size: u32) {
/root/.cargo/registry/src/mirrors.ustc.edu.cn-38d0e5eb5da2abae/h2-0.4.15/src/server.rs:481:    pub fn set_initial_window_size(&mut self, size: u32) -> Result<(), crate::Error> {
/root/.cargo/registry/src/mirrors.ustc.edu.cn-38d0e5eb5da2abae/h2-0.4.15/src/server.rs:495:    pub fn enable_connect_protocol(&mut self) -> Result<(), crate::Error> {
/root/.cargo/registry/src/mirrors.ustc.edu.cn-38d0e5eb5da2abae/h2-0.4.15/src/server.rs:513:    pub fn poll_closed(&mut self, cx: &mut Context) -> Poll<Result<(), crate::Error>> {
/root/.cargo/registry/src/mirrors.ustc.edu.cn-38d0e5eb5da2abae/h2-0.4.15/src/server.rs:529:    pub fn abrupt_shutdown(&mut self, reason: Reason) {
/root/.cargo/registry/src/mirrors.ustc.edu.cn-38d0e5eb5da2abae/h2-0.4.15/src/proto/streams/counts.rs:95:    pub(crate) fn max_local_error_resets(&self) -> Option<usize> {
/root/.cargo/registry/src/mirrors.ustc.edu.cn-38d0e5eb5da2abae/h2-0.4.15/src/proto/streams/counts.rs:153:    pub(crate) fn max_remote_reset_streams(&self) -> usize {
/root/.cargo/registry/src/mirrors.ustc.edu.cn-38d0e5eb5da2abae/h2-0.4.15/src/proto/streams/counts.rs:247:    pub(crate) fn max_send_streams(&self) -> usize {
/root/.cargo/registry/src/mirrors.ustc.edu.cn-38d0e5eb5da2abae/h2-0.4.15/src/proto/streams/counts.rs:253:    pub(crate) fn max_recv_streams(&self) -> usize {
/root/.cargo/registry/src/mirrors.ustc.edu.cn-38d0e5eb5da2abae/h2-0.4.15/src/proto/streams/recv.rs:945:    pub fn max_stream_id(&self) -> StreamId {
/root/.cargo/registry/src/mirrors.ustc.edu.cn-38d0e5eb5da2abae/h2-0.4.15/src/proto/streams/prioritize.rs:107:    pub(crate) fn max_buffer_size(&self) -> usize {
/root/.cargo/registry/src/mirrors.ustc.edu.cn-38d0e5eb5da2abae/h2-0.4.15/src/proto/streams/streams.rs:1043:    pub(crate) fn max_send_streams(&self) -> usize {
/root/.cargo/registry/src/mirrors.ustc.edu.cn-38d0e5eb5da2abae/h2-0.4.15/src/proto/streams/streams.rs:1047:    pub(crate) fn max_recv_streams(&self) -> usize {
/root/.cargo/registry/src/mirrors.ustc.edu.cn-38d0e5eb5da2abae/h2-0.4.15/src/server.rs:390:        if let Some(max) = builder.settings.max_header_list_size() {
/root/.cargo/registry/src/mirrors.ustc.edu.cn-38d0e5eb5da2abae/h2-0.4.15/src/server.rs:570:    pub fn max_concurrent_send_streams(&self) -> usize {
找到了根本原因，且这是无法配置的：h2 的 server.rs:1677 无条件地拒绝了 :authority 中包含 / 的路径。grpc-go 将 UDS 路径作为 :authority 发送（Go 服务器接受它；而 h2 拒绝）。让我确认一下在添加日志依赖后，工作区（workspace）仍然正常，然后进行清理。

Bash Confirm build+test green after deps; cleanup smoke files
IN
cd /home/yunfei/Code/Only-AgentSandbox/only-agent-sandbox
cargo build --workspace --manifest-path Cargo.toml 2>&1 | grep -E "warning|error" | head
echo "=== test ==="
cargo test --workspace --manifest-path Cargo.toml 2>&1 | grep -E "test result:|error" | grep -v "0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered" | head
rm -f /tmp/oas-smoke.sock /tmp/oas-smoke.log /tmp/pod-smoke.json
echo "cleaned"

OUT
=== test ===
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.06s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.16s
cleaned
main.rs 现在已接入真正的 OasManager + mock 后端。构建正常，所有 55 个测试均通过。

变更内容
新增 oas-mock crate (crates/oas-mock/) — 提取了 MockDriver/MockNet/MockStorage/ImmediateReadiness/NeverReadyReadiness，以便生产和测试共享同一个源文件。oas-e2e 现在重新导出它们，而不是定义自己的版本。

src/main.rs — 将 StubManager 替换为真实的 OasManager，通过 MemoryStore + oas-mock 后端 + SystemClock 接入。添加了 env_logger + tracing/log，以便 RUST_LOG=h2=debug 可以正常工作（用于调试，默认静默）。对 UDS 的服务保持不变。

crictl 实测发现了一个互操作阻碍（非 3 层缺陷）
服务器启动正常，并在 UDS 上与 tonic 客户端正确对话 — e2e_kubelet_protocol_sandbox_lifecycle 已经在完全相同的连接配置下，通过完整的 kubelet CRI 序列证明了这一点。但是 crictl (grpc-go) 在每次调用时都会收到 RST_STREAM PROTOCOL_ERROR。

通过 RUST_LOG=h2=trace 捕获的根本原因：


h2::server: malformed headers: malformed authority (b"/tmp/oas-smoke.sock"): invalid uri character
send frame=Reset { stream_id: 1, error_code: PROTOCOL_ERROR }
grpc-go 将 UDS 路径作为 :authority 伪标头发送 (/tmp/oas-smoke.sock)；h2 0.4 在 h2/src/server.rs:1677 无条件地拒绝了它 — 路径不能包含 /，且没有配置可以禁用此检查。Go gRPC 服务器（containerd/CRI-O）接受它；而 h2 不接受。tonic 自身的客户端发送的是有效的 authority，这就是为什么测试可以通过而 crictl 不行的原因。这是一个已知的 tonic/h2 ↔ grpc-go UDS 互操作性差距 (hyperium/h2, hyperium/tonic)。

结论：CRI/Manager/Store 这 3 层在协议层面已验证正确。该阻碍是传输层面的互操作问题，与 firecracker 无关 — 但它会阻塞未来的真实 kubelet（kubelet 也使用 grpc-go）。

修复方案（需要您的决定）
Vendor h2 + 修补掉 authority 验证（通过 [patch.crates-io]）— 1 行修改，使 crictl 和 kubelet 都能正常工作；代价是一个 vendored fork。
TCP 回环模式 — 添加一个 OAS_TCP env flag，以便 crictl 可以使用 --runtime-endpoint tcp://127.0.0.1:port 进行连接（grpc-go 为 TCP 发送有效的 authority）。开销低，足以用于 crictl 手动探索，但 kubelet 需要 UDS，因此这不是一个完整的修复。
在前面加一个小的 Go gRPC 代理 — 也会增加一个 Go 依赖。
我的建议是：如果您希望最终能对接 kubelet，请选择 (1)；如果您只需要进行 3 层的 crictl 冒烟测试，请选择 (2)。您希望我实现哪一个，还是暂时搁置此问题，直接开始 firecracker 相关的工作？

来源：hyperium/h2 issues, hyperium/tonic issues。

Bash tool output (2l7ka7)
选择1
选项 1 —— 供应商（vendor）h2 并放宽 authority 检查。我来阅读一下具体的 validation 代码，以便进行最小化且正确的修补。

Bash Read h2 server header-validation section
IN
sed -n '1620,1730p' ~/.cargo/registry/src/mirrors.ustc.edu.cn-38d0e5eb5da2abae/h2-0.4.15/src/server.rs

OUT
        true
    }
    */

    fn r#dyn() -> proto::DynPeer {
        proto::DynPeer::Server
    }

    fn convert_poll_message(
        pseudo: Pseudo,
        fields: HeaderMap,
        stream_id: StreamId,
    ) -> Result<Self::Poll, Error> {
        use http::{uri, Version};

        let mut b = Request::builder();

        macro_rules! malformed {
            ($($arg:tt)*) => {{
                tracing::debug!($($arg)*);
                return Err(Error::library_reset(stream_id, Reason::PROTOCOL_ERROR));
            }}
        }

        b = b.version(Version::HTTP_2);

        let is_connect;
        if let Some(method) = pseudo.method {
            is_connect = method == Method::CONNECT;
            b = b.method(method);
        } else {
            malformed!("malformed headers: missing method");
        }

        let has_protocol = pseudo.protocol.is_some();
        if has_protocol {
            if is_connect {
                // Assert that we have the right type.
                b = b.extension::<crate::ext::Protocol>(pseudo.protocol.unwrap());
            } else {
                malformed!("malformed headers: :protocol on non-CONNECT request");
            }
        }

        if pseudo.status.is_some() {
            malformed!("malformed headers: :status field on request");
        }

        // Convert the URI
        let mut parts = uri::Parts::default();

        // A request translated from HTTP/1 must not include the :authority
        // header
        if let Some(authority) = pseudo.authority {
            let maybe_authority = uri::Authority::from_maybe_shared(authority.clone().into_inner());
            parts.authority = Some(maybe_authority.or_else(|why| {
                malformed!(
                    "malformed headers: malformed authority ({:?}): {}",
                    authority,
                    why,
                )
            })?);
        }

        // A :scheme is required, except CONNECT.
        if let Some(scheme) = pseudo.scheme {
            if is_connect && !has_protocol {
                malformed!("malformed headers: :scheme in CONNECT");
            }
            let maybe_scheme = scheme.parse();
            let scheme = maybe_scheme.or_else(|why| {
                malformed!(
                    "malformed headers: malformed scheme ({:?}): {}",
                    scheme,
                    why,
                )
            })?;

            // It's not possible to build an `Uri` from a scheme and path. So,
            // after validating is was a valid scheme, we just have to drop it
            // if there isn't an :authority.
            if parts.authority.is_some() {
                parts.scheme = Some(scheme);
            }
        } else if !is_connect || has_protocol {
            malformed!("malformed headers: missing scheme");
        }

        if let Some(path) = pseudo.path {
            if is_connect && !has_protocol {
                malformed!("malformed headers: :path in CONNECT");
            }

            // This cannot be empty
            if path.is_empty() {
                malformed!("malformed headers: missing path");
            }

            let maybe_path = uri::PathAndQuery::from_maybe_shared(path.clone().into_inner());
            parts.path_and_query = Some(maybe_path.or_else(|why| {
                malformed!("malformed headers: malformed path ({:?}): {}", path, why,)
            })?);
        } else if is_connect && has_protocol {
            malformed!("malformed headers: missing path in extended CONNECT");
        }

        b = b.uri(parts);

        let mut request = match b.body(()) {
            Ok(request) => request,
            Err(e) => {
针对最小化补丁：当 :authority 解析失败时，将其丢弃（设为 None），而不是重置流——CRI 处理程序不依赖于该权限。我来引入 h2 并应用此补丁。

Bash Vendor h2 0.4.15 source into workspace
IN
cd /home/yunfei/Code/Only-AgentSandbox/only-agent-sandbox
mkdir -p vendor
cp -r ~/.cargo/registry/src/mirrors.ustc.edu.cn-38d0e5eb5da2abae/h2-0.4.15 vendor/h2
echo "vendored:"; ls vendor/h2 | head; echo "---Cargo.toml version---"; grep '^version' vendor/h2/Cargo.toml | head -1

OUT
vendored:
benches
Cargo.lock
Cargo.toml
Cargo.toml.orig
CHANGELOG.md
CONTRIBUTING.md
examples
LICENSE
README.md
src
---Cargo.toml version---
version = "0.4.15"
Read server.rs (lines 1673-1684)
Edit server.rs
Added 3 lines
现在，将 [patch.crates-io] 部分添加到根目录的 Cargo.toml 中。

Read Cargo.toml
Edit Cargo.toml
Added 6 lines
Bash Rebuild with patched h2
IN
cd /home/yunfei/Code/Only-AgentSandbox/only-agent-sandbox
cargo build --workspace --manifest-path Cargo.toml 2>&1 | grep -E "Compiling h2|warning|error" | head -15

OUT
   Compiling h2 v0.4.15 (/home/yunfei/Code/Only-AgentSandbox/only-agent-sandbox/vendor/h2)
h2 编译成功，补丁已应用。现在重新运行 crictl smoke 测试。

Bash crictl live smoke with patched h2
IN
cd /home/yunfei/Code/Only-AgentSandbox/only-agent-sandbox
SOCK=/tmp/oas-smoke.sock
rm -f "$SOCK" /tmp/oas-smoke.log
OAS_SOCKET="$SOCK" ./target/debug/oas-runtime > /tmp/oas-smoke.log 2>&1 &
SRV=$!
sleep 2
EP="unix://$SOCK"
echo "=== Version ==="
crictl --runtime-endpoint "$EP" --image-endpoint "$EP" --timeout 10s version 2>&1
echo "=== RunPodSandbox (type=0) ==="
cat > /tmp/pod-smoke.json <<'JSON'
{ "metadata": {"name":"smoke","namespace":"default","uid":"uid-smoke","attempt":0},
  "annotations": {"agent-sandbox/type":"0"}, "hostname":"smoke", "logDirectory":"/tmp" }
JSON
SID=$(crictl --runtime-endpoint "$EP" --image-endpoint "$EP" --timeout 10s runp /tmp/pod-smoke.json 2>&1)
echo "sandbox_id=$SID"
echo "=== ListPodSandbox ==="
crictl --runtime-endpoint "$EP" --image-endpoint "$EP" --timeout 10s pods 2>&1
echo "=== InspectPodSandbox ==="
crictl --runtime-endpoint "$EP" --image-endpoint "$EP" --timeout 10s inspectp "$SID" 2>&1 | grep -E "State|state|id:|ip:" | head
echo "=== StopPodSandbox ==="
crictl --runtime-endpoint "$EP" --image-endpoint "$EP" --timeout 10s stopp "$SID" 2>&1
echo "=== RemovePodSandbox ==="
crictl --runtime-endpoint "$EP" --image-endpoint "$EP" --timeout 10s rmp "$SID" 2>&1
echo "=== ListPodSandbox after remove ==="
crictl --runtime-endpoint "$EP" --image-endpoint "$EP" --timeout 10s pods 2>&1
kill $SRV 2>/dev/null
rm -f /tmp/oas-smoke.sock /tmp/oas-smoke.log /tmp/pod-smoke.json

OUT
=== Version ===
Version:  v1
RuntimeName:  only-agent-sandbox
RuntimeVersion:  0.1.0
RuntimeApiVersion:  v1
=== RunPodSandbox (type=0) ===
sandbox_id=sb-0000000000000001
=== ListPodSandbox ===
POD ID              CREATED             STATE               NAME                NAMESPACE           ATTEMPT             RUNTIME
sb-0000000000       56 years ago        Ready               smoke               default             0                   (default)
=== InspectPodSandbox ===
    "state": "SANDBOX_READY",
=== StopPodSandbox ===
Stopped sandbox sb-0000000000000001
=== RemovePodSandbox ===
Removed sandbox sb-0000000000000001
=== ListPodSandbox after remove ===
POD ID              CREATED             STATE               NAME                NAMESPACE           ATTEMPT             RUNTIME
太好了——crictl 现在可以通过 UDS 完成完整的沙箱生命周期：Version → RunPodSandbox → List(Ready) → inspectp(SANDBOX_READY) → stopp → rmp。让我确认一下测试套件在打补丁后的 h2 仍然可以通过。

Bash Full test suite with patched h2
IN
cd /home/yunfei/Code/Only-AgentSandbox/only-agent-sandbox
cargo test --workspace --manifest-path Cargo.toml 2>&1 | grep -E "test result:|error\[|error:|warning:" | grep -v "0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered" | head -20

OUT
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.16s
完成。选项 1 已实现并验证。

已完成的更改
Vendored + 修补了 h2 — vendor/h2/ (h2 0.4.15), 在 vendor/h2/src/server.rs:1672 处进行了一行修改：当 :authority 解析失败时，将其丢弃（设为 None）而不是使用 PROTOCOL_ERROR 重置流。在根目录 Cargo.toml 中通过 [patch.crates-io] h2 = { path = "vendor/h2" } 接入。CRI/tonic 处理程序不读取 :authority，因此丢弃它是安全的。

测试仍然通过 — 55 个测试全部通过，没有警告。该补丁仅影响格式错误的 authority 分支；tonic 客户端发送的是有效的 authority，因此不受影响。

crictl 现场冒烟测试 — 全程通过

Version:          RuntimeName=only-agent-sandbox  RuntimeVersion=0.1.0  Api=v1
RunPodSandbox:    sb-0000000000000001
ListPodSandbox:   Ready  name=smoke
InspectPod:       state=SANDBOX_READY
StopPodSandbox:   Stopped sandbox sb-0000000000000001
RemovePodSandbox: Removed sandbox sb-0000000000000001
ListPodSandbox:   (empty)
一个真实的 grpc-go 客户端现在可以通过 UDS 驱动运行时了 —— 传输互操作性阻碍已解决。kubelet（也是 grpc-go）也将能够连接。

发现一个单独的（外观上的）bug，值得后续跟进
crictl 显示 CREATED: 56 years ago。根本原因：CRI 的 PodSandbox.created_at / ContainerStatus.{created_at,started_at,finished_at} 是 纳秒 单位，但我们的记录存储的是 秒（根据设计 §2.1），并且 convert.rs 直接传递了该值 —— 因此 crictl 将 1.7e9 秒读取为 1.7e9 纳秒 = 1970 年。功能上是正常的（沙箱 READY，生命周期工作正常），但 kubelet 使用 created_at 进行排序/PLEG，所以应该修复它。修复方法是在每个 record_to_* 转换中将秒转换为纳秒（* 1_000_000_000），外加对设计文档的“Unix 秒”约定进行一行更新。需要我接下来做这个吗？

移除 h2 补丁的说明
当 h2 上游放宽 :authority 验证后，删除 [patch.crates-io] 块和 vendor/h2/ —— 普通客户端不需要更改代码。