# CNI 插件实现与 OAS 集成汇报

> 范围：`Only-AgentSandbox_Calico`（OAS 底层 runtime，下文简称 **OAS**）中的 CNI 链，
> 以及 `sandbox_sdk_Calico`（sandbox 云服务 / gateway，下文简称 **SDK 侧**）如何消费它产出的 Pod IP。
> 目标：说清“尾部 CNI 插件到底做了什么”，以及“OAS 是怎么把 CNI 跑起来的”。

---

## 0. 一句话结论

OAS 没有自己实现 IPAM/路由，而是**复用标准 CNI 链**：让 Calico（或离线 lab 的 ptp+host-local）先在沙箱 netns 里把 Pod IP `P` 配到 `eth0` 上，再在链尾挂一个自研插件 `oas-vm-net`。这个尾部插件只做两件事——**在同一个 netns 里建一个固定 tap `tapH0`，并用 nft 装 1:1 的 G↔P NAT**，从而让 Firecracker guest（固定 IP `G=172.16.0.2`）能被宿主机/集群用真实的 Pod IP `P` 访问到。OAS 的 Rust runtime 通过一个 Go 写的薄壳 `oas-cni-invoke` 调起整条 conflist，拿到 `P` 作为 CRI 上报的 Pod IP。

---

## 1. 两个仓库的分工

| 仓库 | 角色 | 关键产物 |
|------|------|----------|
| `Only-AgentSandbox_Calico` (OAS) | 沙箱底层 runtime（CRI 实现 + firecracker 编排） | `oas-runtime`（Rust）、CNI 插件 `oas-vm-net` / `oas-cni-invoke`（Go） |
| `sandbox_sdk_Calico` (SDK 侧) | 沙箱云服务：pod-manager 建 Pod、guest-gateway 拨号 | `pod-manager`、`guest-gateway`、guest agent |

数据流上的关键交接点只有一个：**Pod IP `P`**。OAS 负责把它“造”出来并写进 K8s Pod status；SDK 侧的 gateway 读 `status.podIP`，`dial(P:10000)` 连进 guest agent。两边不共享代码，只靠 `P` 这个 IP 间接耦合（方案 A1）。

---

## 2. CNI 链的形状

链定义在 conflist 里，**`oas-vm-net` 永远是最后一个插件**：

生产模板 `Only-AgentSandbox_Calico/only-agent-sandbox/cni/conflist/10-oas-calico.conflist.example`：

```jsonc
{
  "name": "oas-calico",
  "cniVersion": "1.0.0",
  "plugins": [
    { "type": "calico", "ipam": {"type":"calico-ipam"}, "policy":{"type":"k8s"}, ... },  // ① Calico 分配 P，配到 eth0
    {
      "type": "oas-vm-net",        // ② 尾部插件：建 tap + G↔P NAT
      "tapName": "tapH0",
      "tapGateway": "172.16.0.1/30",
      "guestIP": "172.16.0.2",
      "dnsServer": "10.43.0.10"
    }
  ]
}
```

离线 lab 模板 `…/cni/conflist/10-oas-lab.conflist` 把 Calico 换成 `loopback → ptp + host-local(10.244.99.0/24) → oas-vm-net`，无需 K8s 数据面即可冒烟。

CNI 链的语义：前序插件产生 `prevResult`，把 Pod IP `P` 放到 netns 内某接口（通常 `eth0`）上；`oas-vm-net` 读取 `prevResult` 拿到 `P`，再在**同一 netns** 内追加 tap 和 NAT。它不修改 `prevResult`，原样回传。

---

## 3. 尾部插件 `oas-vm-net` 到底做了什么

源码：`Only-AgentSandbox_Calico/only-agent-sandbox/cni/oas-vm-net/`（`main.go` / `tap.go` / `nat.go`）。它是一个标准 CNI plugin（`skel.PluginMainFuncs`，实现 ADD/DEL/CHECK），**要求必须级联**（`conf.RawPrevResult == nil` 直接报错）。

### 3.1 关键常量与配置

```go
type NetConf struct {
    types.NetConf
    TapName    string  // 默认 "tapH0"
    TapGateway string  // CIDR，默认 "172.16.0.1/30"  —— tap 侧网关 Gw
    GuestIP    string  // 裸 IP，默认 "172.16.0.2"   —— guest 固定 IP G
    GuestMAC   string  // 可选
    DNSServer  string  // 可选：把 tapGW:53 重定向到真实 DNS
}
```

约定记号：
- `P` = prevResult 里的 Pod IP（Calico 分配，集群可达）
- `G` = `guestIP` = `172.16.0.2`（Firecracker guest 内静态配置的 eth0 地址，全沙箱固定）
- `Gw` = `tapGateway` 的地址位 = `172.16.0.1`（tap 设备在 netns 侧的地址，guest 的默认网关）

### 3.2 ADD 流程（`cmdAdd`）

进入目标 netns（`ns.GetNS(args.Netns).Do(...)`）后依次：

1. **建 tap**（`ensureTap`，tap.go:10）
   - `netlink.Tuntap{Mode: TUNTAP_MODE_TAP}` 创建 `tapH0`（已存在则复用）；
   - 给它配地址 `Gw/30`（`172.16.0.1/30`）；
   - `LinkSetUp` 拉起。这个 tap 就是 Firecracker 后续 `host_dev_name` 要绑的设备。

2. **开转发**（`enableForward`，tap.go:67）
   - 在 netns 内写 `/proc/sys/net/ipv4/ip_forward = 1`，允许 tap↔eth0 之间转发。

3. **装 NAT**（`setupNAT`，nat.go:19）
   - 先 `nft delete table ip oas_vm`（幂等清场），再 `nft -f -` 灌入一张 `ip oas_vm` 表：

   ```
   chain prerouting  (nat, dstnat)   : ip daddr P  → dnat to G
   chain postrouting (nat, srcnat)   : ip saddr G  → snat to P
   chain forward     (filter)        : ct state established,related accept
                                      ip saddr G accept
                                      ip daddr G accept
   ```

   即 **1:1 双向 NAT**：从集群发往 Pod IP `P` 的包，进 netns prerouting 时 DNAT 成 `G`，再经转发到 tap 进 guest；guest 回包源 `G`，postrouting 时 SNAT 回 `P`，对外仍是 Pod IP。
   - 可选 DNS：若配了 `dnsServer`，再加两条 `prerouting ip daddr Gw udp/tcp dport 53 → dnat to dnsServer:53`，把 guest 指向 tapGW 的 DNS 查询劫持到集群 DNS（CRI 风格）。

4. **回传** `prevResult`（不改 IP/接口），CNI 链结束。

> 物理拓扑（同一个 netns 内）：
>
> ```
>   [Calico 的 veth/eth0: P] ──转发── [tapH0: Gw=172.16.0.1] ──(Firecracker)── [guest eth0: G=172.16.0.2]
>        ↑ nft 在这里把 P↔G 双向 NAT ↑
> ```

### 3.3 DEL / CHECK

- `cmdDel`（main.go:114）：进 netns `teardownNAT()`（`nft delete table ip oas_vm`）+ `deleteTap`，全部 best-effort 幂等；netns 已删也直接返回 nil。
- `cmdCheck`（main.go:141）：校验 tap 存在且 pod/guest IP 非空（`checkDataPlane`）。

### 3.4 关键点

- **tap 是“固定”的**：名字、Gw、G、MAC 全沙箱写死（见配置）。不同沙箱靠不同 netns 隔离，tap 在各自 netns 内都叫 `tapH0`。这跟 Firecracker snapshot restore 侧的约定一致——guest 镜像里 eth0 就是 `172.16.0.2`，MAC 就是 `06:00:AC:10:00:02`，restore 时 `host_dev_name=tapH0` 必须对上。
- **NAT 而非 tc-redirect / bridge**：选了 `nft` 1:1 NAT（落地方案 Path B / A1），没走 kata 的 tc-redirect。这是“Calico 网络配置 → firecracker vm 用的 tap”之间的桥接方式。
- README 注明当前线上以 bash 插件为准、Go 源码是草稿（`cni/README.md` 第 21 行），但功能定义即上述。

---

## 4. OAS 是怎么用 CNI 的

### 4.1 两个 Go 二进制

| 二进制 | 职责 | 源码 |
|--------|------|------|
| `oas-vm-net` | 上述尾部 CNI 插件本体，被 libcni 按 conflist 调起 | `cni/oas-vm-net/` |
| `oas-cni-invoke` | **薄壳执行器**：OAS Rust 不直接链 libcni，而是 spawn 这个 Go 程序跑整条 conflist | `cni/oas-cni-invoke/main.go` |

`oas-cni-invoke` 是 libcni 的命令行封装：
```
oas-cni-invoke add --config <conflist> --netns <path> --id <sid> --ifname eth0 --cni-path /opt/cni/bin
```
内部 `libcni.ConfListFromFile` + `AddNetworkList`/`DelNetworkList`/`CheckNetworkList`，ADD 时把 CNI result JSON 打到 stdout。`install.sh` 把这两个二进制装进 `/opt/cni/bin`。

### 4.2 Rust 侧：NetManager 的 `mode=cni`

核心在 `Only-AgentSandbox_Calico/only-agent-sandbox/crates/oas-net/src/real.rs`，`NetManager` 有两种模式：

- `mode=mvp`：自建 netns + 固定 tap + 自己的 store IPAM，**主机不可达 guest**（Pod IP 只是记账）。
- `mode=cni`：建**空 netns**，调 CNI conflist，尾部 `oas-vm-net` 做 tap + NAT，**Pod IP 集群可达**。

`setup()` → `setup_cni()`（real.rs:86）：

1. `ip netns add oas-<sid>`（建空 netns，不配任何地址）；
2. 调 `cni_add()`（cni.rs:17）：spawn `oas-cni-invoke add …`，把上面的 netns 路径、sid、conflist、cni-bin-dir 全传进去；`oas-cni-invoke` 依次跑 Calico + `oas-vm-net`，回 result JSON；
3. Rust 解析 result JSON（`extract_ip_gw`，cni.rs:95）：取首个 IPv4 作为 `pod_ip=P`、取 `gateway`；
4. 失败则 `ip netns del` 回滚；成功则把 `P`、netns、tap 名、MAC 塞进 `NetConfig` 返回。
   - 注意：`cni_add` 里 `lease.cidr` 用的是配置里的 `pod_cidr`（信息/回退字段），真正权威的 `P` 来自 CNI result。

`teardown()` → `cni_del()`（real.rs:171）：spawn `oas-cni-invoke del`（幂等，soft-fail 仅 warn），再 `ip netns del`。CNI 模式下不走 store IPAM 释放。

### 4.3 配置

`crates/oas-config/src/lib.rs` 的 `NetConfig` 段（lib.rs:20）持有全部所需字段：`mode`、`tap_name`/`tap_gateway`/`tap_prefix`/`guest_ip`/`guest_mac`、`pod_cidr`/`pod_gateway`、以及 CNI 三件套 `cni_conflist` / `cni_bin_dir` / `cni_invoke`（默认 `/etc/cni/net.d/10-oas-calico.conflist`、`/opt/cni/bin`、`/opt/cni/bin/oas-cni-invoke`）。

lab 实跑配置见 `cni/config-cni-lab.toml`：`mode="cni"`、tap/guest 固定值、`cni_conflist` 指向 lab conflist。

### 4.4 与 CRI 上报的衔接

`oas-runtime` 作为 CRI 实现，`RunPodSandbox` 时执行上述网络 setup，拿到 `P` 后通过 CRI status 把 `P` 作为 Pod IP 上报给 kubelet/containerd。所以 `crictl inspectp` 能看到 `status.network.ip = P`——这就是 SDK 侧 gateway 后面要读的那个 `status.podIP`。

---

## 5. 端到端打通：从建沙箱到 gateway 拨号

1. **建沙箱**（OAS 侧）：`crictl runp`（或 kubelet）→ `oas-runtime` → `NetManager.setup(mode=cni)` → 空 netns → `oas-cni-invoke add` → Calico 分 `P` 到 eth0 + `oas-vm-net` 建 `tapH0`+NAT → 上报 Pod IP=`P`。
2. **起 Firecracker**（OAS 侧）：snapshot restore，`host_dev_name=tapH0` 绑到刚才那个 tap；guest eth0 静态 `G=172.16.0.2`，guest agent 监听 `:10000`。
3. **拨号**（SDK 侧，`sandbox_sdk_Calico/guest-gateway`）：
   - `backend_resolve.rs`：读 `BACKEND_POD_NAME` → K8s `pods.get(name).status.pod_ip` → `format!("{P}:10000")`（backend_resolve.rs:35-39）；
   - `dial(P:10000)`：包进 netns 后被 `prerouting DNAT P→G`，落到 guest agent 的 `:10000`。
4. **回包**：guest `:10000` → `postrouting SNAT G→P` → 集群看到的是 `P`，Calico 策略/路由正常生效。

`smoke_fc_cni.sh`（`cni/smoke_fc_cni.sh`）完整复现了这条链（不经 kubelet，crictl 直连 `/run/oas-calico.sock`），校验项就是这条路径的关键证据：
- `ip netns exec oas-<id> ip addr show tapH0 | grep 172.16.0.1`（tap 建好）
- `nft list table ip oas_vm | grep "dnat to 172.16.0.2"`（NAT 装好）
- `echo >/dev/tcp/172.16.0.2/10000`（netns 内直连 guest）
- `echo >/dev/tcp/${POD_IP}/10000`（**宿主机用 Pod IP 拨通，证明 DNAT 生效**）

---

## 6. 两个仓库约定的“禁用项”

`sandbox_sdk_Calico` 默认拒绝联调 hack（`sandbox_sdk_Calico/CALICO_TRACK.md`）：`OAS_NETNS_DIAL`（方案 C，gateway 进 netns 直连 G）、`SKIP_POD_OWNERSHIP` 都会 `exit 2`，除非 `ALLOW_OAS_HACKS=1`。即正式路径**只认 `dial(podIP:10000)`**，不绕过 NAT 直连 guest IP——这正是 `oas-vm-net` 那套 G↔P NAT 存在的意义。

---

## 7. 一张图总结

```
                 ┌──────────────── Only-AgentSandbox_Calico (OAS runtime) ────────────────┐
   crictl/kubelet                                                                    CRI status
        │ RunPodSandbox                                                              podIP = P
        ▼                                                                                 ▲
  NetManager.setup(mode=cni)                                                             │
   ├─ ip netns add oas-<sid>            (空 netns)                                       │
   ├─ spawn oas-cni-invoke add ──┐                                                        │
   │                             ▼                                                        │
   │   libcni 跑 conflist:                                                                 │
   │     ① calico  → 分配 P，配到 eth0                                                    │
   │     ② oas-vm-net (尾部, 自研):                                                       │
   │          ensureTap(tapH0, 172.16.0.1/30)                                             │
   │          ip_forward=1                                                                 │
   │          nft oas_vm: DNAT P→G, SNAT G→P                                              │
   │                             │                                                        │
   ├─< result JSON: P ──────── 解析 pod_ip=P ─────────────────────────────────────────────┘
   │
   └─ Firecracker snapshot restore: host_dev_name=tapH0, guest eth0=G=172.16.0.2, agent :10000

                 ┌──────────── sandbox_sdk_Calico (gateway) ────────────┐
   BACKEND_POD_NAME                                                      
        │                                                                
        ▼                                                                
   K8s pods.get(name).status.pod_ip = P          （上面 OAS 上报的那个 P）
        │                                                                
        ▼                                                                
   dial(P:10000) ──► netns prerouting DNAT P→G ──► guest agent :10000
          ▲                                                              │
          └────────◄─ postrouting SNAT G→P ◄─────────────────────────────┘
```

## 8. 文件索引

| 关注点 | 文件 |
|--------|------|
| 尾部插件本体 | `Only-AgentSandbox_Calico/only-agent-sandbox/cni/oas-vm-net/main.go` · `tap.go` · `nat.go` |
| CNI 执行器 | `Only-AgentSandbox_Calico/only-agent-sandbox/cni/oas-cni-invoke/main.go` |
| conflist 模板 | `…/cni/conflist/10-oas-calico.conflist.example` · `10-oas-lab.conflist` |
| Rust 调 CNI | `Only-AgentSandbox_Calico/only-agent-sandbox/crates/oas-net/src/cni.rs` · `real.rs` |
| 网络配置 | `…/crates/oas-config/src/lib.rs` · `cni/config-cni-lab.toml` |
| 端到端冒烟 | `cni/smoke_fc_cni.sh` · `cni/install.sh` |
| gateway 拨号 | `sandbox_sdk_Calico/guest-gateway/src/backend_resolve.rs` · `oas_netns_dial.rs` |
| 约定/禁用项 | `sandbox_sdk_Calico/CALICO_TRACK.md` |
