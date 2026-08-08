# 项目上下文 / 领域术语表（glossary）

> 仅记录领域术语与边界，不含实现细节。实现决策走 `0docs/adr/`。

## 进程拓扑

- **Runtime（运行时进程）**：对接 kubelet 的 CRI server。无状态调度器——按沙箱 spawn Shim，之后只查状态 / 发停止。Runtime 重启不得影响已存在的 Shim。
- **Shim（沙箱守护进程）**：每沙箱一个进程，与 Runtime 相互独立。**广义职责**：承担一个沙箱从无到有恢复成 Running 并持续看护的全部步骤（jail root 制备 → jailer+firecracker 拉起 → snapshot load → resume → 进程看护）。恢复状态机自包含在 Shim 内，是 Runtime 可无状态化的充要条件。
- **Firecracker / VMM**：Shim 拉起的真实 microVM 进程（经 jailer chroot 沙箱化）。
- **Shim API（ttrpc）**：Runtime 与 Shim 之间的通信契约，走 ttrpc-rust（轻量 gRPC-over-UDS，containerd shim v2 同款传输）。Shim 是 ttrpc server，Runtime 是 client。这是内部契约，不对外暴露给 kubelet。最小三方法：`Create`（同步恢复到 Running/Failed）、`State`（查实际态）、`Stop`（幂等停）。`Create` 同步阻塞到就绪，故 runtime 侧 `wait_ready`/`event_fd` 退役。
- **再发现（Re-discovery）**：Runtime 重启后内存索引全丢，靠**扫 `$run_base_dir/oas-shim-*.sock`** 重建 `sandbox_id → shim socket` 映射，重连 ttrpc client（driver 不依赖 store；socket 目录即"哪些 shim 活着"的事实源）。store ↔ driver 的孤儿/降级对账归 §4.6 reconcile，MVP 不做。
- **进程独立性**：Runtime 用 `Command` + `pre_exec(setsid)` 拉 Shim，脱离 Runtime session；stdio 重定向到 shim 日志文件。jailer `--daemonize` 使 firecracker 被 reparent 到 init——故 **Shim 崩溃 ≠ firecracker 崩溃**，firecracker 会成孤儿继续跑。
- **可重启 + 幂等 Shim**：`Create` 语义为"确保 VM 在跑且由我管理"——shim 进入时自查 jail root 是否已有活着的 firecracker，有则 re-attach、无则全量 restore，故 `Create` 对同 id 多次调用等价。Runtime 遇 RPC 失败/socket 断 → 用同 sandbox_id re-spawn shim（幂等 re-attach）+ 有界重试。
- **身份校验（防 PID 复用）**：以 **mount namespace inode** 为锚点（jailer chroot 必建独立 mntns，netns 不可靠）。shim 在 create 完成时写 `$ROOT/shim.meta`（`fc_pid`/`mntns_inode`/`started_at`/`shim_pid`）；re-attach/应急杀前读候选 pid，`kill(pid,0)` + `/proc/<pid>/ns/mnt` inode 比对 `shim.meta` 双校验。PID 复用的进程在另一 mntns，inode 不命中。
- **应急杀路径（deterministic resource release）**：`Stop`/`delete_vm` 先试 shim ttrpc Stop；shim 不可达时 runtime 直读 `$ROOT/shim.meta` + `firecracker.pid`，过身份校验后杀 firecracker、清 jail root、删 socket。杀前必校验，防杀错进程。

## 部署路径与 Shim 二进制

存在**两个不同的 Shim 二进制**，分接两条互斥的顶层路径，二者**不嵌套、不互相驱动**：

- **Runtime-driven Shim（Path A，`oas-driver::shim::ShimImpl`）**：kubelet → `oas-runtime`(CRI server) → Shim 守护进程，走自定义 ttrpc 契约 `Create`/`State`/`Stop`（见上「Shim API」）。即上文「Shim」词条所指。
- **containerd-driven Shim（Path B，`oas-ctrd-shim`）**：kubelet → containerd(CRI) → `containerd-shim-oas-v2` 守护进程，走 containerd v2 Task/Sandbox 协议。不经 `oas-runtime`，不被 `RealDriver` 驱动。
- **Restore Core（`vm_core`，待提取）**：两条路径**共享的**进程无关恢复原语——materialize / jailer+firecracker 拉起 / snapshot-load+resume / re-attach / cleanup / `find_fc_pid` / mntns 身份校验。既非 ttrpc server 也非状态机，仅是 VM bring-up 原语集。Path A 的 `ShimImpl` 与 Path B 的 `RealVm` 各自把它包成自己的对外接口（ttrpc `Create/State/Stop` vs `SandboxVm` trait）。当前这批逻辑仍内联在 `oas-driver/src/shim.rs`，尚未抽成独立模块。

## 恢复（Restore）

- **Materialize（物化）**：把 per-VM 可写 ext4 / 云盘拷进 jail root 的固定 jail 内路径（如 `/data.ext4`），让 snapshot 记录的盘路径在新 jail 里仍然命中——而非用 Firecracker drive path override（API 不支持）。
- **Bundle（恢复模板）**：一个自包含的 snapshot 模板目录，位于 `$artifacts_dir/snapshots/<bundle>/`，内含固定名四件套：`vmlinux`(kernel) / `rootfs.ext4`(只读 base rootfs) / `vmstate`(CPU/设备状态) / `mem`(内存镜像)。type 表以 bundle 目录名引用，不同 bundle 恢复出不同沙箱。kernel 跟 bundle 走（非全局），vcpu/mem 已固化在 vmstate 里、restore 时不设 machine-config。
- **配置加载**：`Config::Default`（MVP 硬编码）+ `Config::load(path)`（TOML 文件在则读、否则 Default）为扩展性接缝；runtime `--config <path>` 并把同一 `--config` 透传给 shim，两边读同一份单一事实源。
- **网络模型（MVP）**：每沙箱独立 netns；oas-net 在其中建固定名 tap（约定 `tap0`）+ 网关 IP。bundle 烘焙时 `PUT /network-interfaces/net1` 挂 virtio-net（`iface_id:"net1"`、固定 `guest_mac`、`host_dev_name`=固定 tap 名）。restore 时 jailer `--netns <netns_path>` 让 firecracker 进 netns，firecracker 自动开同名 tap——**不用 `network_overrides`**（netns 隔离使固定 tap 名天然每沙箱唯一；override 留给未来 CNI 改名）。guest 静态 IP（MVP 全沙箱相同，如 `172.16.0.2/30`、网关 `172.16.0.1`），SSH 经 `ip netns exec <netns> ssh <guest_ip>`。NAT/出网留后期。
- **网络接口（向前兼容 CNI）**：`create_vm` 的 `netns_path: &str` 升级为网络 spec `{ netns_path, tap_name }`（manager 已有 `net_cfg.tap_name`，原先漏传）。MVP 中 `tap_name` 恒为固定常量、shim 不用于 override；CNI 落地时改 shim 实现即可，proto 不动。
- **L2 桥接（tc-redirect / mirred）**：tap **无 IP**，guest 直接持有 PodIP；`mirred egress redirect` 在 qdisc ingress 层 `TC_ACT_STOLEN` 窃包，绕过 netfilter。kata `tcfilter` 即此模型，要求 guest 侧有通道（vsock+agent）获知 PodIP。
- **L3 NAT 数据面**：tap 持网关 IP（`172.16.0.1/30`），guest 固定 IP（`172.16.0.2/30`），netfilter `prerouting` DNAT PodIP→guestIP、`postrouting` SNAT guestIP→PodIP。guest **无需知晓 PodIP**。OAS 采纳的模型。
- **数据面互斥性**：tap 有 IP（可路由 + NAT）与 tap 无 IP（L2 redirect）是同一轴的两端，**不可在同一 tap 上叠加**——`mirred` 在 qdisc 层窃包，早于 netfilter NAT 链，会让 NAT 永不触发。选 NAT 还是 tc-redirect，由「guest 是否需要知晓 PodIP」一题决定。
- **tapH0 归属（Path B）**：由 `oas-ctrd-shim` 在 jailer spawn 前、于 pod netns 内创建 tapH0 并安装 NAT（shell out `ip netns exec` + `nft`）；Calico 作纯 CNI 只产 `eth0`(PodIP/32)。`oas-vm-net` CNI 插件保留不用、不删除。解 ADR-0010 遗留的 tap 归属问题。
