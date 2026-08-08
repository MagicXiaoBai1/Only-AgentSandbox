---
status: accepted
---

# In-VM exec 扩展接口：数据 seam + Task 默认，不在 SandboxVm 加方法

当前阶段不做 VM 内 exec（`ctr t exec` / 真实 exit code），但预留扩展 seam：`vm_core` 的恢复产物 `VmHandle` 携带 `guest: Option<GuestEndpoint>`（`Vsock{cid,port}` / `UnixSock{path}`），现恒为 `None`——snapshot 烘焙 guest agent 后再填充。API 层 seam 复用 containerd `Task::exec` 的 trait 默认（返回 NOT_FOUND），不向 `SandboxVm` trait 新增 exec 方法。

刻意不现在加 `SandboxVm::exec`：此刻猜 exec 的请求/响应形状（流式、超时、exit code 语义）几乎必错，而 trait 方法一旦落地就成了 RealVm 的公共契约，改起来要波及所有调用方。传输句柄（`GuestEndpoint`）记录了未来 exec *如何*到达 guest，却不提前承诺 RPC 形状；真正实现 in-VM exec 时，`RealVm` 暴露一个聚焦 guest-ops 的独立 trait，`TaskService::exec` 路由过去，与 `SandboxVm` 解耦。
