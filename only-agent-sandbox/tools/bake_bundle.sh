#!/usr/bin/env bash
# 烘焙一个 net-enabled bundle：产出 $ARTIFACTS_DIR/snapshots/<bundle>/{vmlinux,rootfs.ext4,vmstate,mem}。
#
# 流程（复用 experiments/snap_double_shot 已验证的 create 侧）：jailer+firecracker A →
# logger/boot-source/machine-config/drives/network-interfaces/net1 →
# InstanceStart → Pause → snapshot/create → 把 vmlinux/rootfs.ext4/vmstate/mem 拷进 bundle。
#
# 烘焙出的 snapshot 含 virtio-net（iface_id=net1, host_dev_name=$TAP_NAME, guest_mac=$FC_MAC），
# 故 restore 时 firecracker 会自动连 netns 内同名 tap。guest 镜像须已配 eth0 静态 IP + sshd。
#
# 用法：
#   bake_bundle.sh <bundle> [vcpu] [mem_mib]
#   只读 rootfs + rw data：bake_bundle.sh base-1 2 1024
#   可写 CoW rootfs：ROOTFS_WRITABLE=1 INCLUDE_DATA_DRIVE=0 bake_bundle.sh base-1-rw 2 1024
#
# 环境变量（默认值见下，可覆盖）：
set -euo pipefail
set -x
exec 3>&1

BUNDLE="${1:?usage: bake_bundle.sh <bundle> [vcpu] [mem_mib]}"
VCPU="${2:-2}"
MEM="${3:-1024}"
[[ "$BUNDLE" =~ ^[A-Za-z0-9._-]+$ ]] || { echo "invalid bundle name: $BUNDLE" >&2; exit 2; }

ROOTROOT="${ROOTROOT:-/home/yunfei/workspace/snap_double_shot}"
FC="${FC:-$ROOTROOT/bin/firecracker}"
JAILER="${JAILER:-$ROOTROOT/bin/jailer}"
KERNEL="${KERNEL:-$ROOTROOT/vm_resourse/vmlinux}"
ROOTFS="${ROOTFS:-$ROOTROOT/vm_resourse/rootfs.ext4}"
DATA_SRC="${DATA_SRC:-$ROOTROOT/vm_resourse/data-a.ext4}"   # 烘焙用占位可写盘
ARTIFACTS="${ARTIFACTS:-/var/lib/oas/artifacts}"
BASE="${BASE:-/home/yunfei/Code/Only-AgentSandbox/tmp/oas-bake}"          # jailer chroot-base-dir（烘焙临时）
UID_FC="${UID_FC:-1234}"
GID_FC="${GID_FC:-1234}"
TAP_NAME="${TAP_NAME:-tapH0}"
FC_MAC="${FC_MAC:-06:00:AC:10:00:02}"
ROOTFS_WRITABLE="${ROOTFS_WRITABLE:-0}"
INCLUDE_DATA_DRIVE="${INCLUDE_DATA_DRIVE:-1}"

case "$ROOTFS_WRITABLE:$INCLUDE_DATA_DRIVE" in
  0:0|0:1|1:0|1:1) ;;
  *) echo "ROOTFS_WRITABLE and INCLUDE_DATA_DRIVE must be 0 or 1" >&2; exit 2 ;;
esac

if [[ "$ROOTFS_WRITABLE" == 1 ]]; then
    ROOTFS_READ_ONLY=false
else
    ROOTFS_READ_ONLY=true
fi

ID_A="bake-$BUNDLE"
ROOT_A="$BASE/firecracker/$ID_A/root"
SOCK_A="$ROOT_A/run/firecracker.socket"
BUNDLE_DIR="$ARTIFACTS/snapshots/$BUNDLE"
BUNDLE_STAGE="$ARTIFACTS/snapshots/.${BUNDLE}.tmp.$$"

echo "==> baking bundle $BUNDLE (vcpu=$VCPU mem=${MEM}M rootfs_writable=$ROOTFS_WRITABLE data_drive=$INCLUDE_DATA_DRIVE) into $BUNDLE_DIR"
echo "$ROOT_A"

[[ ! -e "$BUNDLE_DIR" ]] || {
    echo "refusing to overwrite existing bundle: $BUNDLE_DIR" >&2
    echo "choose a new bundle name after verifying the existing artifact" >&2
    exit 1
}
mkdir -p "$ARTIFACTS/snapshots"
mkdir "$BUNDLE_STAGE"

# 烘焙需要一个 netns + tap 供 snapshot 创建时 net1 挂上。临时建一个。
NETNS="bake-$$"
ip netns add "$NETNS" 2>/dev/null || true
ip netns exec "$NETNS" ip tuntap add dev "$TAP_NAME" mode tap 2>/dev/null || true
ip netns exec "$NETNS" ip addr add "172.16.0.1/30" dev "$TAP_NAME" 2>/dev/null || true
ip netns exec "$NETNS" ip link set "$TAP_NAME" up 2>/dev/null || true

cleanup() {
    set +e
    kill "$(cat "$ROOT_A/firecracker.pid" 2>/dev/null)" 2>/dev/null || true
    rm -f "$SOCK_A"
    ip netns del "$NETNS" 2>/dev/null || true
    rm -rf "$BUNDLE_STAGE"
    # rm -rf "$BASE/firecracker/$ID_A"
}
trap cleanup EXIT

# 准备 vm A 的 jail root。
mkdir -p "$ROOT_A"
# 烘焙阶段也要求显式 reflink，避免在非 XFS/btrfs 上悄悄退化为全量复制。
cp --reflink=always "$KERNEL" "$ROOT_A/vmlinux"
cp --reflink=always "$ROOTFS" "$ROOT_A/rootfs.ext4"
if [[ "$INCLUDE_DATA_DRIVE" == 1 ]]; then
    cp --reflink=always "$DATA_SRC" "$ROOT_A/data.ext4"
fi
chown -R "$UID_FC:$GID_FC" "$ROOT_A/"
chmod 0777 "$ROOT_A"
chmod 0444 "$ROOT_A/vmlinux"
if [[ "$ROOTFS_WRITABLE" == 1 ]]; then
    chmod 0666 "$ROOT_A/rootfs.ext4"
else
    chmod 0444 "$ROOT_A/rootfs.ext4"
fi
if [[ "$INCLUDE_DATA_DRIVE" == 1 ]]; then
    chmod 0666 "$ROOT_A/data.ext4"
fi

# 启动 jailer + firecracker A（进烘焙 netns）。
echo "启动 jailer + firecracker A（进烘焙 netns）"

"$JAILER" \
  --id "$ID_A" \
  --exec-file "$FC" \
  --uid "$UID_FC" \
  --gid "$GID_FC" \
  --chroot-base-dir "$BASE" \
  --new-pid-ns \
  --daemonize \
  --netns "/var/run/netns/$NETNS" \
  -- --api-sock run/firecracker.socket

for _ in $(seq 1 50); do [ -S "$SOCK_A" ] && break; sleep 0.1; done
test -S "$SOCK_A"

curl -fsS -X PUT --unix-socket "$SOCK_A" \
  --data "{\"log_path\":\"/fc.log\",\"level\":\"Debug\",\"show_level\":true,\"show_log_origin\":true}" \
  "http://localhost/logger"

curl -fsS -X PUT --unix-socket "$SOCK_A" \
  --data "{\"kernel_image_path\":\"./vmlinux\",\"boot_args\":\"keep_bootcon console=ttyS0\"}" \
  "http://localhost/boot-source"

curl -fsS -X PUT --unix-socket "$SOCK_A" \
  --data "{\"vcpu_count\":$VCPU,\"mem_size_mib\":$MEM}" \
  "http://localhost/machine-config"

curl -fsS -X PUT --unix-socket "$SOCK_A" \
  --data "{\"drive_id\":\"rootfs\",\"path_on_host\":\"./rootfs.ext4\",\"is_root_device\":true,\"is_read_only\":$ROOTFS_READ_ONLY}" \
  "http://localhost/drives/rootfs"

if [[ "$INCLUDE_DATA_DRIVE" == 1 ]]; then
    curl -fsS -X PUT --unix-socket "$SOCK_A" \
      --data "{\"drive_id\":\"data\",\"path_on_host\":\"./data.ext4\",\"is_root_device\":false,\"is_read_only\":false}" \
      "http://localhost/drives/data"
fi

# virtio-net：iface_id=net1, host_dev_name=$TAP_NAME, guest_mac 固定。
curl -fsS -X PUT --unix-socket "$SOCK_A" \
  --data "{\"iface_id\":\"net1\",\"guest_mac\":\"$FC_MAC\",\"host_dev_name\":\"$TAP_NAME\"}" \
  "http://localhost/network-interfaces/net1"

curl -fsS -X PUT --unix-socket "$SOCK_A" \
  --data '{"action_type":"InstanceStart"}' "http://localhost/actions"

sleep 2

curl -fsS -X PATCH --unix-socket "$SOCK_A" \
  --data '{"state":"Paused"}' "http://localhost/vm"

curl -fsS -X PUT --unix-socket "$SOCK_A" \
  --data '{"snapshot_type":"Full","snapshot_path":"./vmstate","mem_file_path":"./mem"}' \
  "http://localhost/snapshot/create"

# 拷出 bundle 四件套到临时目录，校验后原子发布。restore 只会看到完整 bundle。
cp --reflink=always "$ROOT_A/vmlinux" "$BUNDLE_STAGE/vmlinux"
cp --reflink=always "$ROOT_A/rootfs.ext4" "$BUNDLE_STAGE/rootfs.ext4"
cp --reflink=always "$ROOT_A/vmstate" "$BUNDLE_STAGE/vmstate"
cp --reflink=always "$ROOT_A/mem" "$BUNDLE_STAGE/mem"
if [[ "$ROOTFS_WRITABLE" == 1 ]]; then
    touch "$BUNDLE_STAGE/rootfs.writable"
fi
(
    cd "$BUNDLE_STAGE"
    sha256sum vmlinux rootfs.ext4 vmstate mem > SHA256SUMS
)
FC_VERSION=$("$FC" --version 2>/dev/null | head -n1 || true)
cat > "$BUNDLE_STAGE/manifest.json" <<EOF
{
"schema_version": 1,
"bundle": "$BUNDLE",
"firecracker_version": "${FC_VERSION//\"/}",
"vcpu": $VCPU,
"mem_mib": $MEM,
"rootfs_writable": $([[ "$ROOTFS_WRITABLE" == 1 ]] && echo true || echo false),
"data_drive": $([[ "$INCLUDE_DATA_DRIVE" == 1 ]] && echo true || echo false),
"tap_name": "$TAP_NAME",
"guest_mac": "$FC_MAC",
"created_at_utc": "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
}
EOF
"$(dirname "$0")/verify_bundle.sh" "$BUNDLE_STAGE"
chown -R "$(id -u):$(id -g)" "$BUNDLE_STAGE"
chmod 0444 "$BUNDLE_STAGE/vmlinux" "$BUNDLE_STAGE/rootfs.ext4"
if [[ -f "$BUNDLE_STAGE/rootfs.writable" ]]; then
    chmod 0444 "$BUNDLE_STAGE/rootfs.writable"
fi
mv "$BUNDLE_STAGE" "$BUNDLE_DIR"

echo "==> bundle baked: $BUNDLE_DIR"
ls -lh "$BUNDLE_DIR"
