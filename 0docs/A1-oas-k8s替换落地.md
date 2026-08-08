# A1 落地说明：专用沙箱节点 + CNI/PodIP + RuntimeClass oas

> 状态：契约 + 同节点 PodIP 附着 + guest 出向 NAT + Create 等 agent + bake 等 agent 已落地；跨节点真 CNI 与网关 E2E 仍待联调。

## 已落地

### Only-AgentSandbox
- CRI `Status` 广告 `runtime_handlers=[{name:oas}]`
- `RunPodSandbox` 仅接受 `""` / `oas`
- `SandboxRecord.runtime_handler` / `host_veth` 落库
- 白名单含 `guest-agent`
- `oas-net`：
  - host veth：PodIP → netns `eth0`；主机侧挂 `pod_gateway`；netns default route；主机 `route replace <pod_ip> dev <host_veth>`
  - netns 内 DNAT：`PodIP:10000 → guest_ip:10000`
  - guest 出向：netns FORWARD + MASQUERADE guest CIDR；主机 MASQUERADE pod CIDR
  - 配置：`enable_host_veth`、`pod_iface`、`guest_agent_port`、`enable_guest_egress`、`wait_guest_agent`、`guest_agent_wait_secs`
- shim：`fresh_restore` 在 snapshot load 后可选等待 `guest_ip:guest_agent_port`（netns 内 TCP）再报 Running
- 工具：
  - `tools/inject_guest_agent.sh`：注入 `/opt/guest_agent` + systemd/rc.local；补 `/etc/resolv.conf`（8.8.8.8）
  - `tools/bake_bundle.sh`：`GUEST_AGENT_BIN` 时等 agent TCP 就绪再 Pause/snapshot（失败则拒绝烘焙）

### sandbox_sdk（K8s API 编排保留）
- `RuntimeClass oas` + `sandbox.io/runtime=oas`
- pod-manager 默认模板切到 `oas`
- `guest/scripts/export-oas-agent.sh`：导出 guest_agent 供 OAS bake
- Installation / deploy / verify 脚本更新

## 仍待真机

1. 跨节点：接入真实 CNI 插件（当前 host-veth 主要保证同节点 gateway→PodIP）
2. 用含 guest-agent 的 bundle 跑通：`create → Ready → gateway /ping`（建议先 `TEST_KEEP=1 ./tools/test_shim.sh` 再测 `:10000`）
3. 单节点 Path B/C 或专用沙箱节点上的 SDK E2E
## 建议联调命令（两仓库绝对路径）

```bash
# 1) 在 sandbox_sdk 导出 guest_agent
cd /home/gsc/sandbox_sdk
./guest/scripts/export-oas-agent.sh
# 产物：/home/gsc/sandbox_sdk/guest/oas-export/guest_agent

# 2) 在 Only-AgentSandbox 烘焙 bundle（显式传绝对路径，不要相对路径混用）
cd /home/yunfei/Code/Only-AgentSandbox/only-agent-sandbox
GUEST_AGENT_BIN=/home/gsc/sandbox_sdk/guest/oas-export/guest_agent \
  ./tools/bake_bundle.sh base-1 2 1024
```

或一行：

```bash
GUEST_AGENT_BIN=/home/gsc/sandbox_sdk/guest/oas-export/guest_agent \
  /home/yunfei/Code/Only-AgentSandbox/only-agent-sandbox/tools/bake_bundle.sh base-1 2 1024
```

## 验收

- `POST /pod/create` → Pod Ready 且有 `podIP`
- 同节点 `curl $POD_IP:10000` / gateway `/ping` 成功
- 控制面仍走 runc；沙箱走 oas RuntimeClass
