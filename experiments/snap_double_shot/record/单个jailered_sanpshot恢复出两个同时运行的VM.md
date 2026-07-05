以下为全章通用变量，实际执行前请按环境修改：
source ../sh/env.sh
```bash
export LOGFILE=./firecracker.log

export FC=/home/yunfei/workspace/snap_double_shot/bin/firecracker
export JAILER=/home/yunfei/workspace/snap_double_shot/bin/jailer
export KERNEL=/home/yunfei/workspace/snap_double_shot/vm_resourse/vmlinux
export ROOTFS=/home/yunfei/workspace/snap_double_shot/vm_resourse/rootfs.ext4

export BASE=/home/yunfei/workspace/snap_double_shot/work_cwd
export USB_FC=1234
export SID_FC=1234

export ID_A=vm-a
export ID_B=vm-b

export ROOT_A=$BASE/firecracker/$ID_A/root
export ROOT_B=$BASE/firecracker/$ID_B/root

export SOCK_A=$ROOT_A/run/firecracker.socket
export SOCK_B=$ROOT_B/run/firecracker.socket

export DATA_A=/home/yunfei/workspace/snap_double_shot/vm_resourse/data-a.ext4
export DATA_B=/home/yunfei/workspace/snap_double_shot/vm_resourse/data-b.ext4

export SNAP_VSTATE=$ROOTA/snapshot/vestate
export SNAP_BIOS=$ROOTA/snapshot/bsn
```


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

curl -X PUT --unix-socket "${SOCK_A}" \
    --data "{
        \"log_path\": \"${LOGFILE}\",
        \"level\": \"Debug\",
        \"show_level\": true,
        \"show_log_origin\": true
    }" \
    "http://localhost/logger"

KERNEL_BOOT_ARGS=" keep_bootcon console=ttyS0"


curl -X PUT --unix-socket "${SOCK_A}" \
    --data "{
        \"kernel_image_path\": \"./vmlinux\",
        \"boot_args\": \"${KERNEL_BOOT_ARGS}\"
    }" \
    "http://localhost/boot-source"

curl --unix-socket "${SOCK_A}" -i  \
  -X PUT 'http://localhost/machine-config' \
  -H 'Accept: application/json'            \
  -H 'Content-Type: application/json'      \
  -d '{
           "vcpu_count": 2,
           "mem_size_mib": 1024
  }'

curl -X PUT --unix-socket "${SOCK_A}" \
    --data "{
        \"drive_id\": \"rootfs\",
        \"path_on_host\": \"./rootfs.ext4\",
        \"is_root_device\": true,
        \"is_read_only\": true
    }" \
    "http://localhost/drives/rootfs"

curl --unix-socket "$SOCK_A" -i \
  -X PUT "http://localhost/drives/data" \
  -H "Content-Type: application/json" \
  -d '{
    "drive_id": "data",
    "path_on_host": "./data.ext4",
    "is_root_device": false,
    "is_read_only": false
  }'

curl --unix-socket "$SOCK_A" -i \
  -X PUT "http://localhost/actions" \
  -H "Content-Type: application/json" \
  -d '{
    "action_type": "InstanceStart"
  }'


curl --unix-socket "$SOCK_A" -i \
  -X PATCH "http://localhost/vm" \
  -H "Content-Type: application/json" \
  -d '{
    "state": "Paused"
  }'

curl --unix-socket "$SOCK_A" -i \
  -X PUT "http://localhost/snapshot/create" \
  -H "Content-Type: application/json" \
  -d '{
    "snapshot_type": "Full",
    "snapshot_path": "/vmstate",
    "mem_file_path": "/mem"
  }'

ps aux | grep -i firecracker | grep -v grep
kill -9 1337770

```


### 启动 vm b
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
sudo chmod 0600 "$ROOT_B/data.ext4"
```

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