# Firecracker Jailer Snapshot 恢复时更换可写 EXT4 宿主路径实验计划

## #1. 实验目标

验证以下命题：

1. Firecracker snapshot 中保存的 `virtio-block` 磁盘路径是 `==jail 内路径==`，例如 `/data.ext4`。
2. 恢复 snapshot 前，如果在新的 jail root 中把另一个宿主 EXT4 文件 `materialize` 到同一个 jail 内路径 `/data.ext4`，恢复后的 microVM 能打开并使用这个新的 backing file。
3. Firecracker 当前 snapshot load API 不提供 block drive path override；如果 snapshot 盘记录的是 `/data.ext4`，恢复时不能通过 `/snapshot/load` 参数把它改成 `/new-data.ext4`。  
   正确方法是让新 jail root 里的内容仍存在 `/data.ext4`。

### 实验形态：

```
***text
第一次启动：
    jailer + Firecracker (vm-a)
    /data.ext4 -> 宿主文件 data-a.ext4
    启动 microVM，在 guest 内写入 marker
    pause, create snapshot
    关闭 vm-a

第二次恢复：
    jailer + Firecracker (vm-b)
    /data.ext4 -> 宿主文件 data-b.ext4
    load snapshot, resume
    验证 guest 看到的是 data-b.ext4 对应的内容
```

#### 关键问题：

- **核心思想**可以是：`data-a.ext4` 和 `data-b.ext4` 可以是完全不同的宿主文件。
- `==jail 内路径不固定==`：snapshot 中记录的 `/data.ext4` 必须在恢复 VM 的 jail root 下仍存在。

---

## #2. 实验前提

### 前提准备：

1. 已编译的 Firecracker 和 jailer：
   ```
   ***bash
   FC=/usr/bin/firecracker
   JAILER=/usr/bin/jailer
   ```

2. 可启动的 guest kernel 和只读 rootfs：
   ```
   ***bash
   KERNEL=/var/lib/oas/artifacts/vmlinux
   ROOTFS=/var/lib/oas/artifacts/rootfs.ext4
   ```

3. guest 内具备基本工具：
   - 挂 mount `/dev/vdb` 到 `/data`。
   - 能通过 serial 或 SSH 执行命令。
   - 有 `sync`、`mount`、`umount`、`cat`、`echo`。

4. 宿主支持：
   ```
   /dev/vcm
   - jailer 所需权限，通常需要 root。
   - curl
   - mks.ext4
   - truncate
   - 可选：`e2fsck`, `resize2fs`
   ```

5. 如果需要网络验证，请准备 `netns` 和 `TAP`，暂只通过 serial 控制 guest，可省略网络。

---

## #3. 目录和变量约定

以下为全章通用变量，实际执行前请按环境修改：

```bash
***bash
export FC=/usr/bin/firecracker
export JAILER=/usr/bin/jailer
export KERNEL=/var/lib/oas/artifacts/vmlinux
export ROOTFS=/var/lib/oas/artifacts/rootfs.ext4

export BASE=/prv/oas-jailer-exp
export USB_FC=1234
export SID_FC=1234

export ID Arms=a
export ID Arms=g
export ID Arms=b
export ID HBS=vm-neg

export ROOT_A=$BASE/firecracker/$ID_A/rest
export ROOT_B=$BASE/firecracker/$ID_B/rest
export ROOT_MBS=$BASE/firecracker/MB_HBS/rest

export SOCK_A=$ROOT_A/run/firecracker.socket
export SOCK_B=$ROOT_B/run/firecracker.socket
export SOCK_MBS=$ROOT_MBS/run/firecracker.socket

export DATA_A=$ROOT_A/data-a.ext4
export DATA_B=$ROOT_B/data-b.ext4

export SNAP_VSTATE=$ROOTA/snapshot/vestate
export SNAP_BIOS=$ROOTA/snapshot/bsn
```

### 创建工作目录：

```bash
***bash
sudo mkdir -p "$WORK/snapshot" "$ROOT_A" "$ROOT_B"
```

---

## #4. 关键路径设计

本实验固定 snapshot 内的 jail 路径：

```
***text
/vmlinux
/rootfs.ext4
/data.ext4
/vmstate
/neon
```

### 宿主路径映射：

```
***text
第一次启动：
    $ROOT_A/data.ext4 -> $DATA_A 的内容

第二次恢复：
    $ROOT_B/data.ext4 -> $DATA_B 的内容
```

> **注意**：这里的箭头可以通过 `copy`、`reflink`、`hardlink` 实现。为了实验可重复，建议 `copy/reflink`，不建议两个 VM 共享同一个可写 EXT4 文件。

---

## #5. 准备两个不同的可写 EXT4 文件

### 创建 `data-a.ext4`，大小 1 GiB：

```bash
***bash
sudo mkdir -p "$WORK"
sudo truncate -s 1G "$DATA_A"
sudo mkfs.ext4 -F "$DATA_A"
```

### 创建 `data-b.ext4`，大小同样为 1 GiB。**主实验只验证“宿主 backing file 路径/内容改变”，不混入“容量变化”这个额外变量**：

```bash
***bash
sudo truncate -s 1G "$DATA_B"
sudo mkfs.ext4 -F "$DATA_B"
```

> 可选：在宿主上写入不同 marker，便于恢复后识别。  
> 如果 EXT4 文件没有分区表，可以用 loop 挂载 marker：

```bash
***bash
sudo mkdir -p /mnt/oas-data-a /mnt/oas-data-b
sudo mount -o loop "$DATA_A" /mnt/oas-data-a
echo "host-marker-from-data-a" | sudo tee /mnt/oas-data-a/host_marker.txt
sudo umount /mnt/oas-data-a

sudo mount -o loop "$DATA_B" /mnt/oas-data-b
echo "host-marker-from-data-b" | sudo tee /mnt/oas-data-b/host_marker.txt
sudo umount /mnt/oas-data-b
```

---

## #6. 第一次启动前准备 jail root A

把 kernel、rootfs、data disk 放到 A 的 jail root 中，使用固定 jail 内路径：

```bash
***bash
sudo mkdir -p "$ROOT_A"

sudo cp --reflink=auto "$KERNEL" "$ROOT_A/vmlinux"
sudo cp --reflink=auto "$ROOTFS" "$ROOT_A/rootfs.ext4"
sudo cp --reflink=auto "$DATA_A" "$ROOT_A/data.ext4"

sudo chmod 700 "$ROOT_A"
sudo chmod 640 "$ROOT_A/vmlinux" "$ROOT_A/rootfs.ext4"
sudo chmod 600 "$ROOT_A/data.ext4"

***bash
sudo ls -l "$ROOT_A"
```

应看到：
```
***text
vmlinux
rootfs.ext4
data.ext4
```

---

## #7. 启动 jailer + Firecracker A

### 启动 Firecracker A：

```bash
***bash
sudo "$JAILER" \
  --id "$ID_A" \
  --exec-file "$FC" \
  --uid "$UID_FC" \
  --gid "$GID_FC" \
  --chroot-base-dir "$BASE" \
  --new-pid-ns \
  --daemonize \
  --api-sock run/firecracker.socket
```

### 等待 API socket：

```bash
***bash
for i in $(seq 1 50); do
  [ -S "$SOCK_A" ] && break
  sleep 0.1
done
test -S "$SOCK_A"
```

### #8. 配置并启动 microVM A

#### 配置 machine：

```bash
***bash
curl --unix-socket "$SOCK_A" -i \
  -X PUT "http://localhost/machine-config" \
  -H "Content-Type: application/json" \
  -d '{
    "vcpu_count": 1,
    "mem_size_mib": 512
  }'
```

#### 配置 boot source（这里 rootfs 是 `/dev/vda`）：

```bash
***bash
curl --unix-socket "$SOCK_A" -i \
  -X PUT "http://localhost/boot-source" \
  -H "Content-Type: application/json" \
  -d '{
    "kernel_image_path": "/vmlinux",
    "boot_args": "console=ttyS0 reboot=k panic=1 pci=off"
  }'
```

#### 配置只读 rootfs（这里 rootfs 是 `/dev/vda`）：

```bash
***bash
curl --unix-socket "$SOCK_A" -i \
  -X PUT "http://localhost/drives/rootfs" \
  -H "Content-Type: application/json" \
  -d '{
    "drive_id": "rootfs",
    "path_on_host": "/rootfs.ext4",
    "is_root_device": true,
    "is_read_only": true
  }'
```

#### 配置可写 data disk（这里 data disk 是 `/dev/vdb`）：

```bash
***bash
curl --unix-socket "$SOCK_A" -i \
  -X PUT "http://localhost/drives/data" \
  -H "Content-Type: application/json" \
  -d '{
    "drive_id": "data",
    "path_on_host": "/data.ext4",
    "is_root_device": false,
    "is_read_only": false
  }'
```

#### 启动 VM：

```bash
***bash
curl --unix-socket "$SOCK_A" -i \
  -X PUT "http://localhost/actions" \
  -H "Content-Type: application/json" \
  -d '{
    "action_type": "InstanceStart"
  }'
```

#### 验证：

- VM 启动成功。
- guest 中能看到 `/dev/vda` 和 `/dev/vdb`。
- `/dev/vdb` 大小 1 GiB。

```bash
lsblk -d /dev/vdb
dmesg | grep data
mount /dev/vdb /data
cat /data/host_marker.txt
echo "guest-marker-from-vm-a" > /data/guest_marker.txt
sync
```

预期：

```text
host-marker-from-data-a
```

## # 9. 创建 snapshot

为了降低磁盘一致性风险，建议在 guest 内先执行：

```bash
### bash
sync
umount /data
```

如果不能 `umount`，至少执行：

```bash
### bash
sync
```

暂停 VM：

```bash
### bash
curl --unix-socket "$SOCK_A" -i \
  -X PATCH "http://localhost/vm" \
  -H "Content-Type: application/json" \
  -d '{
    "state": "Paused"
  }'
```

创建 snapshot，输出文件位于 A 的 jail root：

```bash
### bash
curl --unix-socket "$SOCK_A" -i \
  -X PUT "http://localhost/snapshot/create" \
  -H "Content-Type: application/json" \
  -d '{
    "snapshot_type": "Full",
    "snapshot_path": "/vmstate",
    "mem_file_path": "/mem"
  }'
```

复制 snapshot artifact 到宿主实验目录：

```bash
### bash
sudo cp --reflink=auto "$ROOT_A/vmstate" "$SNAP_VMSTATE"
sudo cp --reflink=auto "$ROOT_A/mem" "$SNAP_MEM"
sudo chown "$(id -u):$(id -g)" "$SNAP_VMSTATE" "$SNAP_MEM" || true
```

验证：

```bash
### bash
ls -lh "$SNAP_VMSTATE" "$SNAP_MEM"
```

## # 10. 关闭 VM A

优雅关闭方式取决于 guest 能力，可选方式：
1. 通过 guest 执行 `poweroff`。
2. 直接杀掉 Firecracker 进程。

实验中可以直接根据 pid 文件清理：

```bash
### bash
sudo cat "$ROOT_A/firecracker.pid" || true
sudo kill "$(sudo cat "$ROOT_A/firecracker.pid")" || true
```

清理 socket 残留：

```bash
### bash
sudo rm -f "$SOCK_A"
```

> **注意**：不要删除 `$SNAP_VMSTATE` 和 `$SNAP_MEM`。

## # 11. 第二次恢复前准备 jail root B

关键步骤：把 **同一个宿主 EXT4 文件系统** 放到 **同一个 jail 内路径** `/data.ext4`。

```bash
### bash
sudo mkdir -p "$ROOT_B"

sudo cp --reflink=auto "$SNAP_VMSTATE" "$ROOT_B/vmstate.src"
sudo cp --reflink=auto "$SNAP_MEM" "$ROOT_B/mem.src"

sudo cp --reflink=auto "$KERNEL" "$ROOT_B/vmlinux"
sudo cp --reflink=auto "$ROOTFS" "$ROOT_B/rootfs.ext4"

# 这里应使用 DATA_B，而不是 DATA_A。
# 但进入 jail 后的路径仍然是 /data.ext4。
sudo cp --reflink=auto "$DATA_B" "$ROOT_B/data.ext4"

sudo chown -R "$UID_FC:$GID_FC" "$ROOT_B/"
sudo chmod 0700 "$ROOT_B/"
sudo chmod 0400 "$ROOT_B/vmstate.src" "$ROOT_B/mem.src"
sudo chmod 0400 "$ROOT_B/vmlinux" "$ROOT_B/rootfs.ext4"
sudo chmod 0400 "$ROOT_B/data.ext4"
```

验证：

```bash
### bash
ls -lh "$ROOT_B/data.ext4" "$ROOT_B/data.ext4"
```

> 输出示例：
> ```
> -rw-r--r-- 1 1000 1000 1.6GB ...
> -rw-r--r-- 1 1000 1000 1.6GB ...
> ```

验收：

```bash
sudo ls -lh "$ROOT_A/data.ext4" "$ROOT_B/data.ext4"
```

预期：
- `$ROOT_A/data.ext4` 约 1.6GB。
- `$ROOT_B/data.ext4` 约 1.6GB。

这说明宿主上的实际 EXT4 文件路径/内容已更改。

## # 12. 启动 jailer + Firecracker B

```bash
### bash
sudo "$JAILER" \
  --id "$ID_B" \
  --exec-file "$FC" \
  --uid "$UID_FC" \
  --gid "$GID_FC" \
  --chroot-base-dir "$BASE" \
  --new-pid-ns \
  --daemonize \
  --api-sock run/firecracker.socket
```

等待 API socket：

```bash
### bash
for i in $(seq 1 50); do
  [ -S "$SOCK_B" ] && break
  sleep 0.1
done
test -S "$SOCK_B"
```

## # 13. 从 snapshot 恢复 VM B

调用 snapshot load：

```bash
### bash
curl --unix-socket "$SOCK_B" -i \
  -X PUT "http://localhost/snapshot/load" \
  -H "Content-Type: application/json" \
  -d '{
    "snapshot_path": "/vmstate.src",
    "mem_backend": {
      "backend_type": "File",
      "backend_path": "/mem.src"
    },
    "track_dirty_pages": false,
    "resume_vm": true
  }'
```

如果 snapshot 中有网络设备，且 tap 名称需要变化，需添加 `network_overrides`：

```json
{
  "network_overrides": [
    {
      "iface_id": "eth0",
      "host_dev_name": "tap0"
    }
  ]
}
```

> 本实验的磁盘替换不依赖 `network_overrides`。

## # 14. 恢复后验证

guest 内验证：

```bash
### bash
lsblk -b /dev/vdb
mkdir -p /data
mount /dev/vdb /data
cat /data/host_marker.txt
ls -l /data
```

预期：
1. `/dev/vdb` 大小约 1.6GB。
2. `cat /data/host_marker.txt` 输出：
   ```
   host-marker-from-data-b
   ```
3. 如果 `data-b.ext4` 是新文件，通常不存在 A 中写入的：
   ```
   guest_marker.txt
   ```

这说明恢复后的 VM 使用的是 `$ROOT_B/data.ext4`，而不是 `$ROOT_A/data.ext4` 或 `$DATA_A`。
