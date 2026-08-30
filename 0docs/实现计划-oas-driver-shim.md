# 实现计划：oas-driver Shim + 自动沙箱恢复（MVP 全链路）

> 领域术语见根目录 `CONTEXT.md`。本文件是实现计划（步骤、文件、风险），不是 glossary。
> 范围：R2——打通"snapshot 恢复 + SSH 进 VM"的 MVP 全链路。

## 0. 架构速览（决策已定）

- **单二进制两模式**：`oas-runtime`（无子命令，serve CRI）/ `oas-runtime shim ...`（shim 模式）。
- **runtime ↔ shim**：ttrpc-rust，最小三方法 `Create`/`State`/`Stop`；`Create` 同步到 Running/Failed。
- **shim 广义职责**：jail root 制备 → jailer+firecracker（`--netns`/`--new-pid-ns`/`--daemonize`）→ `PUT /snapshot/load`（resume）→ 看 fc_pid。可重启 + 幂等（re-attach via mntns inode）。
- **进程独立**：runtime 用 `pre_exec(setsid)` 拉 shim；jailer `--daemonize` 使 firecracker 脱离。runtime 重启不影响 shim。
- **再发现**：runtime 启动扫 `$run_base/oas-shim-*.sock` 重建索引（不依赖 store）。
- **网络**：每沙箱 netns + 固定名 `tap0`；bundle 烘焙 `net1→tap0` virtio-net；restore 不用 network_overrides；guest 静态 IP，`ip netns exec` SSH。NAT 留后期。
- **配置**：`oas-config` crate，`Config::Default`（MVP 硬编码）+ `Config::load(path)`（TOML），runtime `--config` 透传 shim。

## 1. 实现阶段（按依赖序）

### Phase 0 — Bundle 烘焙脚本（前置，无代码依赖）
**产物**：`tools/bake_bundle.sh`（复用 `experiments/snap_double_shot` 已验证流程）。
**职责**：对给定 type（vcpu/mem）执行 create 侧——jailer+firecracker A → `PUT logger/boot-source/machine-config/drives(rootfs ro + data rw)/network-interfaces/net1` → `InstanceStart` → `Pause` → `PUT /snapshot/create`（`/vmstate`+`/mem`）→ 把 `vmlinux/rootfs.ext4/vmstate/mem` 拷成 bundle 四件套到 `$artifacts_dir/snapshots/<bundle>/`。
**范围**：只烘焙 file-only bundle（type 0/1）。type 2 cloud_disk（块设备）不烘焙、不恢复，shim 遇 `has_cloud_disk` 返回明确错误。
**注意**：guest image 内 eth0 静态 IP（`172.16.0.2/30`、gw `172.16.0.1`）、sshd 预装——这是镜像侧前置，脚本假定已具备。

### Phase 1 — `oas-config` crate（新）
**新建** `crates/oas-config/`：
```rust
#[derive(Deserialize, Clone)]
pub struct Config {
    pub firecracker_bin: PathBuf,
    pub jailer_bin: PathBuf,
    pub chroot_base_dir: PathBuf,
    pub jailer_uid: u32,
    pub jailer_gid: u32,
    pub artifacts_dir: PathBuf,       // 下含 snapshots/<bundle>/
    pub run_base_dir: PathBuf,        // shim socket + netns
    pub log_dir: PathBuf,
    pub store_path: PathBuf,          // redb
    pub cri_socket: PathBuf,
    pub net: NetConfig,               // tap_name/tap_gateway/guest_ip/guest_mac/pod_cidr/pod_gateway
    pub types: Vec<SandboxType>,
}
#[derive(Deserialize, Clone)]
pub struct SandboxType {
    pub type_id: u8, pub bundle: String,
    pub vcpu: u32, pub mem_mib: u32,
    pub has_rw_layer: bool, pub has_cloud_disk: bool,
    pub image_whitelist: Vec<String>,
}
impl Default for Config { /* MVP 硬编码值 */ }
impl Config {
    pub fn load(path: &Path) -> Self { /* TOML 在则读，否则 Default */ }
    pub fn bundle_dir(&self, bundle: &str) -> PathBuf { self.artifacts_dir.join("snapshots").join(bundle) }
}
```
依赖：`serde`、`toml`。

### Phase 2 — ttrpc proto + oas-driver 骨架
**改** `crates/oas-driver/Cargo.toml`：加 `tokio`、`ttrpc`、`prost`、`nix`、`hyper`（或 `ureq`）、`oas-config`、`oas-types`、`async-trait`；build-dep `ttrpc-codegen`、`tonic-prost-build`（按 ttrpc-rust 实际工具链定）。
**新建** `crates/oas-driver/proto/shim.proto`：
```proto
service Shim {
  rpc Create(CreateRequest) returns (CreateResponse);
  rpc State(StateRequest) returns (StateResponse);
  rpc Stop(StopRequest) returns (StopResponse);
}
message CreateRequest {
  string sandbox_id = 1;
  string bundle_dir = 2;          // 宿主绝对路径，shim 自取四件套
  string netns_path = 3;
  string tap_name = 4;            // MVP 恒 tap0，CNI 兼容位
  string rw_layer_path = 5;       // 可空
  string cloud_disk_dev = 6;      // MVP 不支持，传空
  uint32 jailer_uid = 7;
  uint32 jailer_gid = 8;
  string chroot_base_dir = 9;
  string firecracker_bin = 10;
  string jailer_bin = 11;
}
message CreateResponse { string state = 1; string error = 2; }
message StateRequest { string sandbox_id = 1; }
message StateResponse { string state = 1; }   // Running/Stopped/Failed
message StopRequest { string sandbox_id = 1; }
message StopResponse {}
```
> `CreateRequest` 把恢复所需的全部参数从 runtime（已合并 Config+type 表）push 给 shim，shim 不读 type 表。
**新建** `build.rs`：ttrpc 代码生成。
**保留** `lib.rs`（trait + 类型）。`event_fd` 参数从 `create_vm` 签名删除（见 Phase 6）。

### Phase 3 — `oas-driver/firecracker.rs`（HTTP-over-UDS 薄 client）
shim 内部用。方法：`put_logger`、`put_snapshot_load`、`get_info`（身份校验用）。
**实现**：hyper + 自定义 `UnixConnector`（UDS），HTTP/1.1。或 `ureq` + `UnixSocket`（同步更简单，shim 内可 block_on）。**推荐 hyper**（异步、与 tokio 一致，h2 已 vendor）。
**风险**：firecracker API 走 UDS + HTTP/1.1，需验证 hyper UnixConnector 路径写法。

### Phase 4 — `oas-driver/shim.rs`（shim 侧）
`pub async fn run(cli: ShimCli) -> Result<()>`：
1. `Config::load(cli.config)` → 算 jail root = `$chroot_base/firecracker/<sid>/root`、shim.meta 路径。
2. 起 ttrpc server 监听 `cli.socket`，注册 `Shim` 服务实现。
3. **不立即恢复**——等 `Create` RPC（Q6/P）。

`Create` handler（幂等）：
1. **re-attach 判定**：读 `shim.meta`，若有 `fc_pid` + `kill(pid,0)` + `/proc/<pid>/ns/mnt` inode 命中 → 连已有 `$jail_root/run/firecracker.socket`，返回 Running。
2. **fresh restore**：
   - materialize：`cp`（reflink 优先，降级普通 copy）bundle 四件套到 jail root 固定路径（`/vmlinux`/`/rootfs.ext4`/`/vmstate.src`/`/mem.src`）+ `rw_layer_path`→`/data.ext4`（type 0 无）。
   - `chown -R jailer_uid:gid` + `chmod`（0700/0444/0666，跟随记录）。
   - spawn `jailer --id <sid> --exec-file <fc> --uid --gid --chroot-base-dir <base> --new-pid-ns --netns <netns_path> --daemonize -- --api-sock run/firecracker.socket`。
   - 轮询等 `$jail_root/run/firecracker.socket`（手动文档 §12，超时 → Failed）。
   - `PUT /logger` → `PUT /snapshot/load {snapshot_path:"/vmstate.src", mem_backend:{File,"/mem.src"}, resume_vm:true}`。
   - 成功 → 写 `shim.meta`（fc_pid + `stat(/proc/<pid>/ns/mnt).st_ino` + started_at + shim_pid），起 fc_pid 看护 task（firecracker 死 → 内部态 Stopped），返回 Running。
   - 失败 → 返回 Failed（带原因；是否清 jail root 留 debug，可配）。

`State` handler：返回内部态。
`Stop` handler：过身份校验后 `kill(fc_pid)`，清 jail root，rm socket，shim 进程退出。

**身份/应急辅助函数**（shim 与 runtime 共用，放 `oas-driver/identity.rs`）：`read_shim_meta`/`write_shim_meta`、`verify_fc_pid(pid, mntns_inode)`、`kill_pid`。

### Phase 5 — `oas-driver/driver.rs`（runtime 侧 RealDriver）
```rust
pub struct RealDriver { cfg: Arc<Config>, index: Mutex<HashMap<String, PathBuf /*socket*/>> }
```
- `new(cfg)`：扫 `$run_base/oas-shim-*.sock`，逐个 connect+`State`；活的入索引，stale(ECONNREFUSED)→unlink。
- `create_vm(id, net_spec, type_id, spec)`：算 socket 路径 → `spawn_shim(sid, socket, config, log)`（`Command`+`pre_exec(setsid)`+stdio→`$log_dir/oas-shim-<sid>.log`）→ 等 socket → ttrpc connect → `Create`（同步）→ Ok 入索引 / Err `kill -9 shim` 回滚。
- `get_vm(id)`：connect+`State`；连不上 → re-spawn shim（幂等 re-attach）+ `State`，有界重试（3 次 / 100ms）→ 映射 `VmStatus`（NotFound/Running/Stopped/Failed→Degraded）。
- `list_vm()`：遍历索引逐个 `State`。
- `delete_vm(id)`：先 ttrpc `Stop`；连不上 → **应急杀**（`identity::read_shim_meta` + `verify_fc_pid` → `kill` → 清 jail root → rm socket）；出索引。幂等。
- `spawn_shim`：构造 `oas-runtime shim --config <p> --sandbox-id <sid> --socket <p> --log-file <p>`。

### Phase 6 — `oas-manager` 重构
- `SandboxType`：删 `snapshot_path`+`rootfs_ro`，加 `bundle`；`SandboxTypeTable` 从 `Config.types` 构造（启动加载，不热更）。
- `FirecrackerDriver::create_vm` 签名：`netns_path: &str` → `net: VmNet { netns_path, tap_name }`；删 `event_fd`。`VmSpec` 不变。
- manager `run_sandbox`：`driver.create_vm(&sid, VmNet{netns_path: net_cfg.netns_path, tap_name: net_cfg.tap_name}, type_id, spec)`（原来只传 netns_path，补 tap_name）。
- `wait_ready`：真实现接 `ImmediateReadiness`（no-op，`Create` 已同步就绪）。
- `IdGenerator`：启动从 store seed（扫 `SandboxRecord`，解析 `sb-<hex>` 取 max+1），避免重启 id 碰撞。

### Phase 7 — `oas-net` 真实现
`crates/oas-net/src/real.rs`：`NetManager { cfg }`。
- `setup(sid)`：`ip netns add`（路径 `$run_base/netns/oas-<sid>`，bind-mount 到 `/var/run/netns/` 供 `ip netns` 与 jailer `--netns` 用）→ `ip tuntap add tap0` → `ip link set tap0 netns <ns>` → netns 内 `ip addr add <gateway>/30 dev tap0` + `up`。返回 `NetConfig{netns_path, tap_name:"tap0", pod_ip: IPAM lease, gateway, ...}`。
- `teardown`：`ip netns del`（含 tap），`release_ip`。
- 所有 netns syscall 关 `spawn_blocking`（design doc §3.4，RAII 还原）。
- IPAM：pod_ip 从 store `lease_ip(pod_cidr)`（已有）；tap 网关 IP 固定（`cfg.net.tap_gateway`）。两者解耦——MVP 接受 pod_ip(IPAM) 与 eth0 实际静态 IP 不一致，CNI 后期统一。

### Phase 8 — `oas-storage` 真实现
`crates/oas-storage/src/real.rs`：`StorageManager { cfg }`。
- `provision(sid, type_id, cloud_disk_ref)`：若 `has_rw_layer` → `truncate -s <size>` + `mkfs.ext4 -F` 生成 `$rw_base/<sid>.ext4`，返回 `DiskConfig{rw_layer_path: Some(...), cloud_disk_dev}`。type 0 无 rw 层 → `rw_layer_path: None`。cloud_disk MVP 返回错误或忽略（type 2 不恢复）。
- `cleanup(disk)`：删 rw ext4 文件。幂等。

### Phase 9 — `oas-runtime/main.rs` 装配
- clap：`enum Mode { Runtime(RuntimeCli), Shim(ShimCli) }`，默认 Runtime。
- Runtime 模式：`Config::load` → `RedbStore::open(store_path)` → `RealDriver::new(cfg)`（启动再发现）→ `NetManager`/`StorageManager` 真实现 → `OasManager::new(...)` → serve CRI on `cri_socket`。删 `MemoryStore`/`MockDriver`/`MockNet`/`MockStorage` 装配（mock 退回仅测试用）。
- Shim 模式：`oas_driver::shim::run(cli).await`。

## 2. 验证计划

1. `cargo build` 单二进制。
2. `tools/bake_bundle.sh base-1` 烘焙 type 1 bundle（含 virtio-net）。
3. runtime 模式起，`crictl runp` 一个 type 1 pod：
   - shim 进程出现、firecracker 进程出现、`/proc/<fc>/ns/net` 在沙箱 netns。
   - `ip netns exec oas-<sid> ssh <guest_ip>` 能登入。
4. **runtime 重启**：kill runtime → shim+firecracker 存活 → 重起 runtime → `crictl inspectp` 仍 READY（再发现命中）。
5. **shim 崩溃自愈**：kill shim（不动 firecracker）→ `crictl inspectp` 触发 `get_vm` → runtime re-spawn shim re-attach → VM 仍活、SSH 仍通。
6. **应急杀**：kill shim → `crictl stopp` → runtime 应急杀 firecracker（过 mntns 校验）→ jail root 清、socket 删、无残留 firecracker 进程。
7. 幂等：同 pod_uid 重复 `runp` 不产生第二个 VM。

## 3. 实现期需验证的风险点（不阻塞设计，编码时确认）

- **jailer pid 文件语义**：`--new-pid-ns` 下 `firecracker.pid` 记 host pid 还是 ns 内 pid。若 ns 内 → runtime `/proc/<host-pid>` 对不上，re-attach/应急杀改用 mntns 扫描找 host pid。shim.meta 自记 fc_pid（从 jailer pid 文件读，或 spawn 后扫 mntns）作权威。
- **ttrpc-rust 工具链**：版本、codegen（ttrpc-codegen）、UDS server/client 写法——按 crate 文档对齐。
- **hyper UDS HTTP/1.1 client**：firecracker API 走 UDS，验证 UnixConnector；若坑大，降级 `ureq`+`UnixSocket` 同步。
- **reflink copy**：`cp --reflink=auto` 需 CoW 文件系统（btrfs/xfs）；非 CoW 降级普通 copy（`std::fs::copy`）。
- **tap0 权限**：jailer uid 1234 能否打开 netns 内 tap0——可能需调 tap 所有权/`CAP_NET_ADMIN`，或 firecracker 以 root 起后再降权。记录里 jailer uid=1234 跑通，跟随即可。
- **guest 镜像**：eth0 静态 IP + sshd 是镜像侧前置，非本代码范围，但 MVP 验证依赖它就绪。
- **sandbox_id 作 jailer `--id`**：`sb-<hex>` 含连字符，jailer 接受（实验 `vm-a` 验证）。socket/netns 文件名也用它，文件系统安全。

## 4. 不在 MVP 范围（显式列出）

- cloud_disk（块设备）恢复——type 2 返回错误。
- NAT/出网、CNI 对接（接口已留 `tap_name` 前向兼容）。
- vsock guest agent / 容器原语真实实现（§2.8，仍 no-op）。
- store ↔ driver 孤儿/降级对账（§4.6 reconcile）。
- snapshot 在线烘焙进二进制（用脚本离线烘焙）。
- IdGenerator 跨进程持久化（用 store seed 近似，真正持久化迁 store `meta` 表后续）。
