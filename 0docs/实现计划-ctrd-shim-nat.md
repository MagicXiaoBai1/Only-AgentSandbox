# 实现计划：oas-ctrd-shim 内做 L3 NAT 数据面

> 决策见 [ADR-0012](adr/0012-sandbox-datapath-nat-in-shim-not-tc-redirect.md)。本计划只覆盖 Path B（oas-ctrd-shim + Calico）；Path A（oas-net）与共享 `vm_core` 不动。

## 目标

Calico 作纯 CNI 产 `eth0`(PodIP/32) 于 pod netns；`oas-ctrd-shim` 在 jailer spawn 前于同一 netns 内创建 `tapH0` + 装 nftables NAT，使控制面经 PodIP DNAT 可达 guest（`172.16.0.2`），guest 经 SNAT 出网。guest 无需知晓 PodIP。`oas-vm-net` CNI 插件保留不用。

## 数据面（移植 oas-vm-net `renderNFT` 的 MVP 子集）

```
network ── eth0(PodIP/32) ─[prerouting DNAT PodIP→172.16.0.2]─→ tapH0(172.16.0.1/30) ─→ guest(172.16.0.2)
guest   ── tapH0 ─[postrouting SNAT 172.16.0.2→PodIP]─→ eth0 ─→ network
```

nft 脚本（`nft -f -` 喂入，全部 `ip netns exec <ns>` 内执行）：

```
add table ip oas_vm
flush table ip oas_vm
add chain ip oas_vm prerouting { type nat hook prerouting priority dstnat; policy accept; }
add chain ip oas_vm postrouting { type nat hook postrouting priority srcnat; policy accept; }
add chain ip oas_vm forward { type filter hook forward priority filter; policy drop; }
add rule ip oas_vm prerouting iifname "<eth0>" ip daddr <PodIP> dnat to 172.16.0.2
add rule ip oas_vm prerouting iifname "tapH0" ip daddr 172.16.0.1 udp dport 53 dnat to <DNS>:53
add rule ip oas_vm prerouting iifname "tapH0" ip daddr 172.16.0.1 tcp dport 53 dnat to <DNS>:53
add rule ip oas_vm postrouting oifname "<eth0>" ip saddr 172.16.0.2 snat to <PodIP>
add rule ip oas_vm forward ct state established,related accept
add rule ip oas_vm forward iifname "tapH0" oifname "<eth0>" ip saddr 172.16.0.2 accept
add rule ip oas_vm forward iifname "<eth0>" oifname "tapH0" ip daddr 172.16.0.2 accept
```

MVP 不含 `netdev oas_guard`（MAC 过滤）与 `IngressTCPPorts`（端口 DNAT）——forward 双向放行，访问控制交给 Calico network policy。

tap 创建（`ip netns exec <ns>` 内）：

```
ip tuntap add dev tapH0 mode tap user <jailer_uid> group <jailer_gid>   # 持久 tap，owner=jailer
ip link set tapH0 address <TapMAC> mtu <eth0_MTU> up
ip addr replace 172.16.0.1/30 dev tapH0
sysctl -w net.ipv4.ip_forward=1     # 写 /proc/sys/net/ipv4/ip_forward，netns 内
```

## 插入点（已核对源码）

- **setup**：`RealVm::create`（`crates/oas-ctrd-shim/src/common/vm.rs:276`）的 `spawn_blocking` 闭包内，`core2.create(&inputs)` **之前**调用 `setup_datapath(&inputs.netns_path, &cfg)`。
- **teardown**：`RealVm::stop`（`vm.rs:310`）的 `spawn_blocking` 闭包内，`core.cleanup()` **之后**调用 `teardown_datapath(&netns_path, &cfg)`。需要把 `netns_path` 带进 `RealVm`（create 时存一份，stop 时取）。
- `VmCore`、`MockVm`、`build_restore_inputs` 均不改。

## 新增模块：`crates/oas-ctrd-shim/src/common/netns_nat.rs`

两个幂等函数（全部 shell out，复用 `oas-net` 风格）：

```rust
pub fn setup_datapath(netns_path: &str, cfg: &Config) -> Result<(), String>
pub fn teardown_datapath(netns_path: &str, cfg: &Config) -> Result<(), String>
```

### setup_datapath 流程
1. **发现 PodIP + eth0 MTU**：`ip netns exec <ns> ip -o -4 addr show eth0` → 解析 `PodIP/32`。若 `eth0` 不存在，回落扫描 veth 类型链路（`ip -o link show type veth`）取第一个，记录其名作 `cni_if`。
2. **幂等检查**：`ip netns exec <ns> ip link show tapH0` 存在则跳过 tap 创建（re-attach 场景）；nft 表用 delete-then-create 事务（`delete table ip oas_vm` 失败可忽略 → `add table` + 规则）。
3. 创建 tapH0（上节命令），设 MAC/MTU/addr/up，开 ip_forward。
4. 渲染 nft 脚本（PodIP/cni_if/DNS 注入），`nft -f -` 喂入。

### teardown_datapath 流程
1. `ip netns exec <ns> nft delete table ip oas_vm`（不存在则忽略）。
2. `ip netns exec <ns> ip link del tapH0`（不存在则忽略）。
3. 全程不 fail-fast：错误 join 返回，但尽力清完。

### shell-out 约定
- 统一 `ip netns exec <ns> <cmd>` 形态（与 `vm_core::wait_tcp_in_netns` 一致），shim 进程自身不 setns。
- 超时 10s（对齐 oas-vm-net `runNFT`）。
- 失败信息带 stderr。

## RealVm 改动

```rust
pub struct RealVm {
    cfg: Arc<Config>,
    core: Mutex<Option<Arc<VmCore>>>,
    netns_path: Mutex<Option<String>>,        // 新增：create 存，stop 取
    exit: ...,
    exit_rx: ...,
}
```

- `create`：`build_restore_inputs` 后存 `netns_path`；`spawn_blocking` 内先 `netns_nat::setup_datapath(&inputs.netns_path, &cfg)`，再 `core2.create(&inputs)`。setup 失败直接返回 Err（不进 restore）。
- `stop`：`spawn_blocking` 内先 `core.cleanup()`，再 `if let Some(ns) = netns { netns_nat::teardown_datapath(&ns, &cfg) }`。teardown 失败仅日志，不影响 stop 返回。

## 配置（`oas-config::NetConfig`）

复用既有字段：`tap_name`(tapH0)、`guest_ip`(172.16.0.2)、tap 网关地址（`172.16.0.1/30`，由 `tap_gateway`+`tap_prefix` 或新字段）、`jailer_uid`/`jailer_gid`（主 Config）。

需确认/新增：
- **TapMAC**（`06:00:ac:10:00:01`）：oas-vm-net 有，oas-config 未见。新增 `NetConfig.tap_mac` 或硬编码常量。
- **DNS resolver**：新增 `NetConfig.dns`（如 `10.96.0.10` 或节点解析器）。需对照 `tools/inject_guest_agent.sh` 写入 guest `resolv.conf` 的指向——若 resolv.conf 直指集群 DNS 且经 egress SNAT 可达，则 DNS DNAT 规则可省；若指 `172.16.0.1` 则必须保留。**待确认。**

## 校验

- **guest agent 探针不变**：`wait_tcp_in_netns` 探 `172.16.0.2:10000` 走 tapH0 子网（`172.16.0.0/30` 直连，不经 NAT），setup 后仍可达。
- **新增集成测试**（`crates/oas-ctrd-shim` 或 `tests/`）：建临时 netns + 假 eth0（veth + PodIP/32），调 `setup_datapath`，断言 tapH0 存在/addr/MAC/owner、`nft list table ip oas_vm` 含 DNAT/SNAT 规则、`ip_forward==1`；调 `teardown_datapath` 断言全清。复用 `oas-vm-net` e2e 的 `installCalicoEth0` 思路。
- **手动联调**：k8s pod（RuntimeClass `agent-firecracker`）→ 进 guest 验证 `ip route`、出网（`curl`）、控制面入站（ssh/端口）。

## 实施步骤（小步可回退）

1. 新增 `netns_nat.rs`：`setup_datapath`/`teardown_datapath` + nft 脚本渲染，单元测试渲染输出。
2. `RealVm` 加 `netns_path` 字段 + setup/teardown 挂载。
3. `oas-config` 补 `tap_mac`/`dns`（确认 resolv.conf 后定 DNS 规则去留）。
4. 集成测试（临时 netns + 假 eth0）。
5. k8s 联调验证双向连通。
6. （延后）按需补 `oas_guard` MAC 过滤与 `IngressTCPPorts`。

## 风险

- **CAP_NET_ADMIN**：已核实 containerd v2 shim 继承 containerd(root) 全量 caps，无 `base_runtime_spec` 限权（`/etc/containerd/config.toml` agent-firecracker 段）。无阻塞。
- **PodIP 发现**：Calico 默认 `eth0`；非默认名时回落扫描 veth。MVP 先支持 `eth0` + 扫描兜底。
- **DNS**：依赖 guest `resolv.conf` 指向，见上「待确认」。
- **re-attach 幂等**：setup 检测 tapH0 已存在则跳过创建；nft 用 delete-then-create。runtime re-spawn shim → re-attach 路径下 tapH0/nft 已在，不重复装。
