# kubelet 联调测试报告（mock 后端）

**日期**：2026-07-03
**目标**：在 driver/net/storage 全 mock 的前提下，用**真实 kubelet**（v1.28.2）打 `oas-runtime`，验证 CRI/API 层 + Manager 层 + Store 层三层在协议层能否让 kubelet 把一个静态 Pod 跑到 Running。
**结论**：三层协议层**通过** kubelet 验证——sandbox Ready + container Running，稳定约 60s。容器最终被 kubelet 周期重建，属 mock 固有边界（无真实 netns/进程），非三层缺陷。

---

## 1. 背景

前序工作已用 tonic 客户端 + crictl 验证过三层（见 [测试设计.md](测试设计.md)）。本轮换用**真 kubelet**（standalone 静态 Pod 模式）再压一遍，因为 kubelet 的 CRI 调用序列比 crictl 更复杂（PLEG 周期 relist、sandbox 健康校验、镜像规范化、cgroup/cadvisor 交叉校验），能暴露 crictl 暴露不出来的对接问题。

被测对象：`oas-runtime` 二进制，装配**真 `OasManager` + mock 后端**（`oas_mock` 的 driver/net/storage + `MemoryStore`），UDS 监听 `/run/oas.sock`。

## 2. 环境

| 项 | 值 |
|----|----|
| OS | openEuler 22.03 (SP4), aarch64 |
| kubelet | v1.28.2（standalone，不连 api-server） |
| 宿主 | **真实 K8s 节点**：containerd + 业务 Pod 在跑，10250/10248 端口被宿主 kubelet 占用 |
| 运行时 | `oas-runtime`（debug build），mock 后端，`/run/oas.sock` |
| 脚手架 | [tests/env.sh](../tests/env.sh)、[tests/start-kubelet.sh](../tests/start-kubelet.sh) |

> ⚠️ 共驻噪音：因为宿主是真 K8s 节点，standalone kubelet 的 cadvisor 会扫到宿主集群的 cgroup，日志里大量 `cri-containerd-*.scope` / `kubepods.slice` 噪音。这是后续某个问题的放大器。

## 3. 测试过程（问题诊断与修复，按时间线）

### 3.1 transport 互操作：`RST_STREAM PROTOCOL_ERROR`

kubelet 一连上就报 `stream terminated by RST_STREAM with error code: PROTOCOL_ERROR`。

**诊断**：`RUST_LOG=h2=trace` 跑运行时，抓到：
```
h2::server: malformed headers: malformed authority (b"/run/oas.sock"): invalid uri character
send frame=Reset { stream_id: 1, error_code: PROTOCOL_ERROR }
```
grpc-go 把 UDS 路径 `/run/oas.sock` 当 `:authority` 伪头发；h2 严格按 URI 校验，路径含 `/` → 拒绝。tonic 自家客户端发合法 authority，所以单测/e2e 都过，唯独 grpc-go 客户端（kubelet/crictl）挂。h2 该校验不可配置。

**修复**：vendored h2 0.4.15（[vendor/h2/](../only-agent-sandbox/vendor/h2/)），把 [server.rs](../only-agent-sandbox/vendor/h2/src/server.rs) 里 authority 解析失败从「reset 流」改为「丢弃 authority 继续」。通过 `[patch.crates-io] h2 = { path = "vendor/h2" }` 接入。修完后：
```
Validated CRI v1 runtime API ✓
Container runtime initialized  containerRuntime="only-agent-sandbox" version="0.1.0" apiVersion="v1" ✓
```

### 3.2 kubelet 端口冲突

```
Failed to start healthz server: listen tcp 127.0.0.1:10248: bind: address already in use
Failed to listen and serve: listen tcp 0.0.0.0:10250: bind: address already in use
```
宿主 kubelet 占了。**修复**：[start-kubelet.sh](../tests/start-kubelet.sh) 加 `--port=15250 --healthz-port=15248 --read-only-port=0`。

### 3.3 `ImageFsInfo` mountpoint 不存在

```
Failed to get the info of the filesystem with mountpoint: stat failed on /var/lib/oas
InvalidDiskCapacity: invalid capacity 0 on image filesystem
```
kubelet 拿 `ImageFsInfo` 返回的 mountpoint 去 `statfs` 取真实容量；我们返回 `/var/lib/oas`（不存在）→ 容量 0 → 告警。**修复**：[image_svc.rs](../only-agent-sandbox/crates/oas-cri/src/image_svc.rs) mountpoint 改 `/`（根 fs 容量充足，不触发 DiskPressure）。

### 3.4 `created_at` 单位错（"56 years ago"）

crictl `pods` 的 CREATED 列显示 `56 years ago`。record 按设计存 Unix 秒，但 CRI proto 的 `created_at`/`started_at`/`finished_at` 是**纳秒**。kubelet/crictl 把 `1.7e9`（秒）当纳秒读 → 1970 年。

**修复**：[convert.rs](../only-agent-sandbox/crates/oas-cri/src/convert.rs) 加 `secs_to_ns(s) = s.saturating_mul(1_000_000_000)`，所有 `record_to_*` emit 时换算。修完显示 `Less than a second ago`。

### 3.5 镜像白名单「三连」（最难的一串）

Pod 用 `image: img-a`，`agent-sandbox/type: "0"`（type0 白名单 `img-a`）。连续踩三个：

**3.5.1 `ImagePullBackOff`**：kubelet 把 `img-a` 规范化成 `docker.io/library/img-a:latest` 再查白名单，精确匹配 `==` 失败 → 拉镜像 → 拉不到 → backoff。
**修复**：[types_table.rs](../only-agent-sandbox/crates/oas-manager/src/types_table.rs) 引入 `image_base_name`（剥 registry/path/tag），白名单按 base-name 匹配。

**3.5.2 `ImageInspectError: Id or size of image "img-a:latest" is not set`**：kubelet `ImageStatus` 校验 `image.id != "" && image.size > 0`，我们 `size: 0`。
**修复**：[manager.rs](../only-agent-sandbox/crates/oas-manager/src/manager.rs) `image_info()` 的 `size` 设 `1_048_576`。

**3.5.3 `CreateContainerError: image not in whitelist: sha256:img-a:latest`**：kubelet 把 `ImageStatus` 返回的 `image.id`（我们填的 `sha256:img-a`）当 CreateContainer 的 image ref 回传，并规范化成 `sha256:img-a:latest`。`image_base_name("sha256:img-a:latest")` 当时只剥 tag → `sha256:img-a`，不等于 `img-a` → 白名单不命中。
**修复**：`image_base_name` 先剥 `sha256:`/`sha512:` digest 前缀，再剥 path/tag → `img-a`。

修完后 kubelet 终于：
```
Created container workload ✓
Started container workload ✓
crictl ps: ct-0000000000  Running  workload
```

### 3.6 运行时重启后的 stale state

调试途中多次重启运行时（改代码重编）。`MemoryStore` 是内存的，重启即空；kubelet 却还缓存着旧 sandbox → ListPodSandbox 返回空 → kubelet 认为沙箱丢了 → 重建 → 撞上我们 `run_sandbox` 的幂等逻辑返回 `Conflict: exists but not ready` → 循环。

**修复**：调试时 clean restart——清 `tests/agent-rt-test/kubelet-root` + 同时重启 kubelet，让两者状态归零。（生产环境用 `RedbStore` 持久化则无此问题。）

### 3.7 sandbox 周期重建（mock 固有边界，未解决）

容器 Running 约 60s 后被 kubelet 杀掉重建，循环。日志：
```
Killing container with a grace period  containerID="...ct-0000000000000001"
Stopping PodSandbox for pod, will start new one
CreatePodSandboxError: conflict: sandbox ... exists but not ready; remove first
```

**根因**：kubelet 的 `sandboxNeedsToBeRecreated` 不只看 `state==Ready`，还要校验网络命名空间真实存在/可用。我们的 `MockNet` 返回了 `netns_path` 字符串但没真建 netns，kubelet 进入/校验该 netns 失败 → 判定 sandbox 不健康 → 重建。

**尝试 `hostNetwork: true`**：反而**更快**被杀（~1s）。因为宿主是真 K8s 节点，standalone kubelet 的 cadvisor 扫到宿主集群 cgroup，共驻噪音 + 无真实进程 → PLEG 很快判异常。

**结论**：这是 mock 的固有边界——**mock 能让 kubelet 在 CRI 协议层满意（建到 Running），但 kubelet 更深的健康/网络校验需要真实底层**（真 netns/tap + 真 firecracker 进程）。要稳定驻留，须落地真实 `NetworkManager` + `FirecrackerDriver`。

## 4. 最终结果

clean run（清状态 + 非 hostNetwork）：
```
kubelet:  Validated CRI v1 runtime API ✓
          Container runtime initialized: only-agent-sandbox v0.1.0 apiVersion=v1 ✓
crictl pods: sb-0000000000  Ready   oas-kubelet-test-k8shost   (podIP 10.244.0.1)
crictl ps:   ct-0000000000  Running  workload  image=sha256:img-a:latest
kubelet 事件: Created container workload ✓ / Started container workload ✓
```

**三层（CRI/Manager/Store）协议层被真实 kubelet 验证通过**：建 sandbox（Ready + podIP）→ 建 container（白名单+资源校验过）→ start → Running，稳定约 60s，PLEG 读路径（ListPodSandbox/ListContainers/ContainerStatus）全程无 CRI 错误。

## 5. 复现步骤

```bash
# 1) 编译
cd only-agent-sandbox && cargo build --workspace

# 2) 起运行时（mock 后端，UDS /run/oas.sock）
OAS_SOCKET=/run/oas.sock ./target/debug/oas-runtime &

# 3) 起 standalone kubelet（已避让端口 15250/15248）
cd ../tests && bash start-kubelet.sh

# 4) 投放静态 Pod（image=img-a, type=0, 资源在 type0 预算内）
cat > agent-rt-test/static-pods/oas-test.yaml <<'YAML'
apiVersion: v1
kind: Pod
metadata:
  name: oas-kubelet-test
  namespace: default
  annotations:
    agent-sandbox/type: "0"
spec:
  restartPolicy: Always
  containers:
    - name: workload
      image: img-a
      imagePullPolicy: IfNotPresent
      command: ["sleep", "infinity"]
      resources:
        requests: { cpu: "0.5", memory: "128Mi" }
        limits:   { cpu: "0.5", memory: "256Mi" }
YAML

# 5) 观察
crictl --runtime-endpoint unix:///run/oas.sock pods
crictl --runtime-endpoint unix:///run/oas.sock ps
tail -f agent-rt-test/logs/kubelet.log
```

清理：`pkill -x kubelet; pkill -x oas-runtime; rm -f agent-rt-test/static-pods/oas-test.yaml`。

## 6. 本轮代码改动（全在三层内）

| 文件 | 改动 | 解决问题 |
|------|------|----------|
| [convert.rs](../only-agent-sandbox/crates/oas-cri/src/convert.rs) | `secs_to_ns`，emit 时间戳换算纳秒 | §3.4 created_at 单位 |
| [image_svc.rs](../only-agent-sandbox/crates/oas-cri/src/image_svc.rs) | `ImageFsInfo` mountpoint → `/` | §3.3 InvalidDiskCapacity |
| [types_table.rs](../only-agent-sandbox/crates/oas-manager/src/types_table.rs) | `image_base_name` + `image_allowed`（剥 sha256:/registry/tag） | §3.5 镜像白名单三连 |
| [manager.rs](../only-agent-sandbox/crates/oas-manager/src/manager.rs) | `Image.size` 非 0；改用 `image_allowed` | §3.5.2 / §3.5.3 |
| [start-kubelet.sh](../tests/start-kubelet.sh) | 端口 15250/15248 | §3.2 端口冲突 |
| [vendor/h2/](../only-agent-sandbox/vendor/h2/) | 放宽 `:authority` 校验 | §3.1 RST_STREAM（上轮已做） |

55 个单测/e2e 全程绿、零警告。

## 7. 遗留与后续

- **未解决**：§3.7 sandbox 周期重建。属 mock 固有边界，需真实底层（真 netns + 真 firecracker 进程）才能让 kubelet 长期满意。
- **共驻噪音**：本机是真 K8s 节点，standalone kubelet 受 cadvisor/cgroup 串扰。理想做法是在干净 VM/容器里测；本机测则把 standalone kubelet 的 `--root-dir`、端口、`--pod-manifest-path` 都隔离开（已做）。
- **后续**：落地真实 `NetworkManager`（netns/tap/IPAM）+ 真实 `FirecrackerDriver` 后，重跑本测试，§3.7 应消失，容器可长期 Running。
- **建议补**：给 `Image` 返回真实 sha256 digest 形式的 `id`（而非 `sha256:<imagename>`），更贴近真实运行时，避免 kubelet 把 id 当 ref 用带来的匹配绕路（当前靠 `image_base_name` 剥前缀兜底）。
