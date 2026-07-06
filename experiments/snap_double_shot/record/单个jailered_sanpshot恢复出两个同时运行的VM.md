以下为全章通用变量，实际执行前请按环境修改：

进入 work_cwd 目录：
1. 配置变量和网络命名空间
```bash
source ../sh/env.sh
bash ../sh/0_setup_iptables_for_vm_a.sh
bash ../sh/0_setup_iptables_for_vm_b.sh
```

2. 准备vm a的启动文件
```bash
sudo mkdir -p "$ROOT_A"

sudo cp --reflink=auto "$KERNEL" "$ROOT_A/vmlinux"
sudo cp --reflink=auto "$ROOTFS" "$ROOT_A/rootfs.ext4"
sudo cp --reflink=auto "$DATA_A" "$ROOT_A/data.ext4"

sudo chmod 0700 "$ROOT_A"
sudo chmod 0400 "$ROOT_A/vmlinux" "$ROOT_A/rootfs.ext4"
sudo chmod 0600 "$ROOT_A/data.ext4"

sudo ls -l "$ROOT_A"
```

### 启动 Firecracker A 打上镜像：

```bash
sudo "$JAILER" \
  --id "$ID_A" \
  --exec-file "$FC" \
  --uid "$UID_FC" \
  --gid "$GID_FC" \
  --chroot-base-dir "$BASE" \
  --new-pid-ns \
  --daemonize \
  -- --api-sock run/firecracker.socket

sleep 0.5

curl -X PUT --unix-socket "${SOCK_A}" \
    --data "{
        \"log_path\": \"${LOGFILE}\",
        \"level\": \"Debug\",
        \"show_level\": true,
        \"show_log_origin\": true
    }" \
    "http://localhost/logger"

sleep 0.5

curl -X PUT --unix-socket "${SOCK_A}" \
    --data "{
        \"kernel_image_path\": \"./vmlinux\",
        \"boot_args\": \"keep_bootcon console=ttyS0\"
    }" \
    "http://localhost/boot-source"

sleep 0.5

curl --unix-socket "${SOCK_A}" -i  \
  -X PUT 'http://localhost/machine-config' \
  -H 'Accept: application/json'            \
  -H 'Content-Type: application/json'      \
  -d '{
           "vcpu_count": 2,
           "mem_size_mib": 1024
  }'

sleep 0.5

curl -X PUT --unix-socket "${SOCK_A}" \
    --data "{
        \"drive_id\": \"rootfs\",
        \"path_on_host\": \"./rootfs.ext4\",
        \"is_root_device\": true,
        \"is_read_only\": true
    }" \
    "http://localhost/drives/rootfs"

sleep 0.5

curl --unix-socket "$SOCK_A" -i \
  -X PUT "http://localhost/drives/data" \
  -H "Content-Type: application/json" \
  -d '{
    "drive_id": "data",
    "path_on_host": "./data.ext4",
    "is_root_device": false,
    "is_read_only": false
  }'

sleep 0.5

curl --unix-socket "$SOCK_A" -i \
  -X PUT "http://localhost/actions" \
  -H "Content-Type: application/json" \
  -d '{
    "action_type": "InstanceStart"
  }'

sleep 0.5

curl --unix-socket "$SOCK_A" -i \
  -X PATCH "http://localhost/vm" \
  -H "Content-Type: application/json" \
  -d '{
    "state": "Paused"
  }'

sleep 0.5

curl --unix-socket "$SOCK_A" -i \
  -X PUT "http://localhost/snapshot/create" \
  -H "Content-Type: application/json" \
  -d '{
    "snapshot_type": "Full",
    "snapshot_path": "/vmstate",
    "mem_file_path": "/mem"
  }'

# ps aux | grep -i firecracker | grep -v grep
# kill -9 1337770

```
### 准备 vm b 的文件
```bash
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
sudo chmod 0600 "$ROOT_B/data.ext4"
```
### 启动 vm b

```bash
sudo "$JAILER" \
  --id "$ID_B" \
  --exec-file "$FC" \
  --uid "$UID_FC" \
  --gid "$GID_FC" \
  --chroot-base-dir "$BASE" \
  --new-pid-ns \
  --daemonize \
  -- --api-sock run/firecracker.socket

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