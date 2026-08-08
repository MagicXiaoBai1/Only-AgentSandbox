---
status: accepted
---

# 两个 Shim 二进制共享 Restore Core（vm_core）

仓库长期并存**两个 per-sandbox 守护进程二进制**，互不嵌套、互不驱动：
Path A 是 `oas-runtime`(CRI) → `oas-driver::shim::ShimImpl`（自定义 ttrpc `Create`/`State`/`Stop`）；
Path B 是 kubelet → containerd(CRI) → `oas-ctrd-shim`（containerd v2 Task/Sandbox 协议）。
两条路径的 firecracker 恢复逻辑（materialize / jailer / snapshot-load+resume / re-attach / cleanup / `find_fc_pid` / mntns 身份校验）**共享同一个进程无关模块 `oas_driver::vm_core`**：`ShimImpl` 把它包成 ttrpc 服务，`RealVm` 把它包成 `SandboxVm` trait 实现。

之所以不统一成单一 daemon：两条路径对接的上层（kubelet 直连 vs containerd）是不同的部署形态，各自独立演进而又必须共享安全关键的恢复原语——分叉这套逻辑会导致「一条路径修了 cleanup 竞态、另一条静默泄漏 firecracker」。`vm_core` 是同步、无 ttrpc、无 condvar 的纯库；`RealVm` 经 `tokio::task::spawn_blocking` 桥接到异步 `SandboxVm`。继续代码中 `决策1-8` 之后的编号。
