# OAS 可写 rootfs 与冷启动验证

## 设计

OAS bundle 默认是不可变模板。带 `rootfs.writable` 标记的 bundle 表示其
Firecracker snapshot 已把 root drive 烘焙为 `is_read_only=false`。恢复时 shim
仍然保留 golden `rootfs.ext4` 为只读，只对每个 sandbox 的 reflink 副本设置
`0666`。因此 `/workspace` 可写，同时不同 sandbox 通过 XFS/btrfs CoW 保持隔离。

没有标记的旧 bundle 继续按 `0444` 恢复，行为不变。

## 烘焙

```bash
sudo ROOTFS_WRITABLE=1 INCLUDE_DATA_DRIVE=0 \
  only-agent-sandbox/tools/bake_bundle.sh base-1-agent-rw 2 1024
```

构建脚本会同时完成以下工作：

- 将 Firecracker root drive 设置为可写；
- 在 bundle 中写入 `rootfs.writable`；
- 保持发布后的 golden `rootfs.ext4` 权限为 `0444`；
- 通过临时目录和校验文件原子发布 bundle。

## 部署

先确认目标目录是可写的 XFS 或 btrfs，然后构建 release binary：

```bash
cd only-agent-sandbox
cargo build --release
sudo ./tools/install_reflink_runtime.sh
```

将目标 type 的 `bundle` 改为 `base-1-agent-rw`，审核配置备份后重启
`oas-runtime`。运行中的 sandbox 不会自动切换 bundle，应先排空或销毁。

## 验证

每次新建 sandbox 后至少验证：

```bash
findmnt -no SOURCE,FSTYPE,OPTIONS /
mkdir -p /workspace/oas-rw-test
echo ok > /workspace/oas-rw-test/result
cat /workspace/oas-rw-test/result
```

宿主机还应确认实例副本与 golden 模板不是同一个文件，并检查 reflink/CoW
物理 extent。连续创建多个 sandbox 时，第一个实例写入的标记不得出现在第二个实例。

冷启动统计以 CRI `RunPodSandbox` 调用开始、guest agent 端口健康为结束点，同时记录
shim 日志中的 materialize、jailer spawn、snapshot load 和 readiness 阶段耗时。不要把
单独的 reflink 文件复制时间当成端到端冷启动时间。

## 回退

停止创建新 sandbox，恢复安装脚本生成的 `/etc/oas/config.toml.bak.*` 和
`/usr/local/bin/oas-runtime.bak.*`，再重启服务。已经由可写 bundle 创建的 sandbox
应先清理，避免新旧 runtime 混用。
