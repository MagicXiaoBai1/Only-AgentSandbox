# Only-AgentSandbox
构建：cargo build -p oas-ctrd-shim
测试：cargo test -p oas-ctrd-shim
仅构建二进制文件：cargo build -p oas-ctrd-shim --bin containerd-shim-oas-v2

## 部署：containerd 透传 pod annotation（必需）

Pod 通过 annotation `agent-sandbox/type` 选择 sandbox bundle type（→
`$artifacts_dir/snapshots/<bundle>/`）。但 containerd CRI **默认不**把任意 pod
annotation 写进 sandbox 容器的 OCI spec（config.json）——只注入它自己的
`io.kubernetes.cri.*`。必须在 **runtime handler** 下配 `pod_annotations`，匹配的
annotation 才会进 config.json，shim 才能读到。

**注意两点**（已对照 containerd 1.6.33 源码确认）：
1. `pod_annotations` 是 **per-runtime-handler** 字段（`Runtime` struct,
   `pkg/cri/config/config.go`），不是全局 CRI 字段 —— 要写在对应 handler 段下，
   不是 `[plugins."io.containerd.grpc.v1.cri"]` 顶层。
2. 匹配用 `path.Match`（**shell glob，不是正则**，见 `pkg/cri/server/helpers.go`
   `getPassthroughAnnotations`）。所以用 `agent-sandbox/*`，不是 `agent-sandbox/.*`
   （后者里 `.` 是字面量，不匹配 `type`）。

测试 pod 的 `runtimeClassName: agent-firecracker`，故配在该 handler 下：

```toml
[plugins."io.containerd.grpc.v1.cri".containerd.runtimes.agent-firecracker]
  runtime_type = "io.containerd.oas.v2"
  pod_annotations = ["agent-sandbox/*"]   # glob: 透传 agent-sandbox/type、cloud-disk、rw-size
```

配好后链路：

```
yaml annotations.agent-sandbox/type
  → kubelet → CRI RunPodSandbox(PodSandboxConfig.annotations)
  → containerd CRI: getPassthroughAnnotations 用 glob 匹配 → 写进 sandbox OCI spec config.json 的 annotations
                   (pkg/cri/server/sandbox_run_linux.go: customopts.WithAnnotation)
  → shim 读 config.json 解析 type_id → cfg.get_type(type_id) → bundle
```

未配 `pod_annotations` 或裸 `ctr run`（不经 CRI）时，shim 收不到该 annotation，
`type_id` 回落 `0`（见 `TaskService::create` 的 `spec.type_id.unwrap_or(0)`）——
节点仍可跑 type 0，但无法按 Pod 选型。

### 完整部署（containerd + RuntimeClass + shim 二进制）

当前 `containerd config dump` 里只有 runc/kata-fc/kata-qemu，**没有** `agent-firecracker`
handler，所以 pod 的 `runtimeClassName: agent-firecracker` 现在无对应实现。按下面三步配齐。

**1) 装 shim 二进制**（containerd 按 `runtime_type=io.containerd.oas.v2` 找 `containerd-shim-oas-v2`）：
```bash
cargo build -p oas-ctrd-shim --release
sudo install -m 0755 -o root -g root \
  target/release/containerd-shim-oas-v2 /usr/local/bin/containerd-shim-oas-v2
# 验证 containerd 能找到：
containerd-shim-oas-v2 -v
```

**2) 给 containerd 加 `agent-firecracker` handler**：编辑 `/etc/containerd/config.toml`，
在 `[plugins."io.containerd.grpc.v1.cri".containerd.runtimes]` 段下（与 runc/kata-fc 并列）
追加（片段见 `tests/agent-rt-test/containerd-oas-handler.toml`）：
```toml
[plugins."io.containerd.grpc.v1.cri".containerd.runtimes.agent-firecracker]
  runtime_type = "io.containerd.oas.v2"
  pod_annotations = ["agent-sandbox/*"]   # glob: 透传 agent-sandbox/type 等
  container_annotations = []
```
重启并确认：
```bash
sudo systemctl restart containerd
containerd config dump | grep -A4 'runtimes\."agent-firecracker"'
```

**3) 建 RuntimeClass**（`handler` 必须等于上面 handler 名）：见
`tests/agent-rt-test/runtimeclass-agent-firecracker.yaml`：
```yaml
apiVersion: node.k8s.io/v1
kind: RuntimeClass
metadata:
  name: agent-firecracker
handler: agent-firecracker
```
```bash
kubectl apply -f tests/agent-rt-test/runtimeclass-agent-firecracker.yaml
```

**4) 端到端验证**（最确证）：起一个带 `agent-sandbox/type` 的 pod，看 config.json：
```bash
kubectl apply -f tests/agent-rt-test/pod-pool/lite.yaml   # runtimeClassName: agent-firecracker + annotation
SID=$(crictl pods --name sb-lite-<ID> -q | head -1)
BUNDLE=$(crictl inspectp "$SID" -o json | jq -r '.status.bundle')
cat "$BUNDLE/config.json" | jq '.annotations["agent-sandbox/type"]'   # 配通→"0"；没配→null
```

配通后链路：`pod(runtimeClassName=agent-firecracker + annotation agent-sandbox/type)`
→ kubelet → CRI RunPodSandbox(runtimeHandler=agent-firecracker, annotations)
→ containerd 用 `agent-firecracker` handler，其 `pod_annotations=["agent-sandbox/*"]` glob 命中
→ 写进 sandbox OCI spec config.json 的 annotations
→ shim 读 config.json → `type_id` → `cfg.get_type(type_id)` → bundle。

