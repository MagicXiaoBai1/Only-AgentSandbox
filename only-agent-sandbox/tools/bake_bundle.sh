#!/usr/bin/env bash
# 烘焙一个 net-enabled bundle：产出 $ARTIFACTS_DIR/snapshots/<bundle>/{vmlinux,rootfs.ext4,vmstate,mem}。
#
# 流程（复用 experiments/snap_double_shot 已验证的 create 侧）：jailer+firecracker A →
# logger/boot-source/machine-config/drives(rootfs ro + data rw)/network-interfaces/net1 →
# InstanceStart → Pause → snapshot/create → 把 vmlinux/rootfs.ext4/vmstate/mem 拷进 bundle。
#
# 烘焙出的 snapshot 含 virtio-net（iface_id=net1, host_dev_name=$TAP_NAME, guest_mac=$FC_MAC），
# 故 restore 时 firecracker 会自动连 netns 内同名 tap。guest 镜像须已配 eth0 静态 IP。
# 若设置 GUEST_AGENT_BIN，会在 snapshot 前把 guest-agent 注入 rootfs 并开机监听 :10000。
#
# 用法：
#   bake_bundle.sh <bundle> [vcpu] [mem_mib]
#   例: bake_bundle.sh base-1 2 1024
#   例: GUEST_AGENT_BIN=/path/to/guest_agent bake_bundle.sh base-1 2 1024
#
# 环境变量（默认值见下，可覆盖）：
set -euo pipefail
set -x
exec 3>&1

BUNDLE="${1:?usage: bake_bundle.sh <bundle> [vcpu] [mem_mib]}"
VCPU="${2:-2}"
MEM="${3:-1024}"

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
GUEST_AGENT_BIN="${GUEST_AGENT_BIN:-}"
GUEST_AGENT_PORT="${GUEST_AGENT_PORT:-10000}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

ID_A="bake-$BUNDLE"
ROOT_A="$BASE/firecracker/$ID_A/root"
SOCK_A="$ROOT_A/run/firecracker.socket"
BUNDLE_DIR="$ARTIFACTS/snapshots/$BUNDLE"

echo "==> baking bundle $BUNDLE (vcpu=$VCPU mem=${MEM}M) into $BUNDLE_DIR"
echo "$ROOT_A"

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
    # rm -rf "$BASE/firecracker/$ID_A"
}
trap cleanup EXIT

# 准备 vm A 的 jail root。
mkdir -p "$ROOT_A"
cp --reflink=auto "$KERNEL" "$ROOT_A/vmlinux"
cp --reflink=auto "$ROOTFS" "$ROOT_A/rootfs.ext4"
cp --reflink=auto "$DATA_SRC" "$ROOT_A/data.ext4"

if [[ -n "$GUEST_AGENT_BIN" ]]; then
  echo "==> injecting guest-agent from $GUEST_AGENT_BIN"
  sudo "$SCRIPT_DIR/inject_guest_agent.sh" "$ROOT_A/rootfs.ext4" "$GUEST_AGENT_BIN" "$GUEST_AGENT_PORT"
fi

# inject 经 sudo 可能把 rootfs 属主改成 root；jailer 需要 uid/gid=$UID_FC。
if ! chown -R "$UID_FC:$GID_FC" "$ROOT_A/" 2>/dev/null; then
  sudo chown -R "$UID_FC:$GID_FC" "$ROOT_A/"
fi
chmod 0777 "$ROOT_A" || sudo chmod 0777 "$ROOT_A"
chmod 0444 "$ROOT_A/vmlinux" "$ROOT_A/rootfs.ext4" || sudo chmod 0444 "$ROOT_A/vmlinux" "$ROOT_A/rootfs.ext4"
chmod 0666 "$ROOT_A/data.ext4" || sudo chmod 0666 "$ROOT_A/data.ext4"

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

curl -sS -X PUT --unix-socket "$SOCK_A" \
  --data "{\"log_path\":\"/fc.log\",\"level\":\"Debug\",\"show_level\":true,\"show_log_origin\":true}" \
  "http://localhost/logger"

curl -sS -X PUT --unix-socket "$SOCK_A" \
  --data "{\"kernel_image_path\":\"./vmlinux\",\"boot_args\":\"keep_bootcon console=ttyS0\"}" \
  "http://localhost/boot-source"

curl -sS -X PUT --unix-socket "$SOCK_A" \
  --data "{\"vcpu_count\":$VCPU,\"mem_size_mib\":$MEM}" \
  "http://localhost/machine-config"

curl -sS -X PUT --unix-socket "$SOCK_A" \
  --data "{\"drive_id\":\"rootfs\",\"path_on_host\":\"./rootfs.ext4\",\"is_root_device\":true,\"is_read_only\":true}" \
  "http://localhost/drives/rootfs"

curl -sS -X PUT --unix-socket "$SOCK_A" \
  --data "{\"drive_id\":\"data\",\"path_on_host\":\"./data.ext4\",\"is_root_device\":false,\"is_read_only\":false}" \
  "http://localhost/drives/data"

# virtio-net：iface_id=net1, host_dev_name=$TAP_NAME, guest_mac 固定。
curl -sS -X PUT --unix-socket "$SOCK_A" \
  --data "{\"iface_id\":\"net1\",\"guest_mac\":\"$FC_MAC\",\"host_dev_name\":\"$TAP_NAME\"}" \
  "http://localhost/network-interfaces/net1"

curl -sS -X PUT --unix-socket "$SOCK_A" \
  --data '{"action_type":"InstanceStart"}' "http://localhost/actions"

GUEST_IP="${GUEST_IP:-172.16.0.2}"
AGENT_WAIT_SECS="${AGENT_WAIT_SECS:-60}"

if [[ -n "$GUEST_AGENT_BIN" ]]; then
  echo "==> waiting for guest-agent at ${GUEST_IP}:${GUEST_AGENT_PORT} (up to ${AGENT_WAIT_SECS}s)"
  ready=0
  for _ in $(seq 1 "$AGENT_WAIT_SECS"); do
    if ip netns exec "$NETNS" python3 -c \
      "import socket; s=socket.create_connection(('${GUEST_IP}',${GUEST_AGENT_PORT}),1); s.close()" \
      2>/dev/null; then
      ready=1
      break
    fi
    if ip netns exec "$NETNS" bash -c "echo >/dev/tcp/${GUEST_IP}/${GUEST_AGENT_PORT}" 2>/dev/null; then
      ready=1
      break
    fi
    sleep 1
  done
  if [[ "$ready" -ne 1 ]]; then
    echo "ERROR: guest-agent not reachable before snapshot; refusing to bake cold image" >&2
    exit 1
  fi
  echo "==> guest-agent ready; pausing for snapshot"
else
  sleep 2
fi

curl -sS -X PATCH --unix-socket "$SOCK_A" \
  --data '{"state":"Paused"}' "http://localhost/vm"

curl -sS -X PUT --unix-socket "$SOCK_A" \
  --data '{"snapshot_type":"Full","snapshot_path":"./vmstate","mem_file_path":"./mem"}' \
  "http://localhost/snapshot/create"

# 拷出 bundle 四件套。
mkdir -p "$BUNDLE_DIR"
cp --reflink=auto "$ROOT_A/vmlinux" "$BUNDLE_DIR/vmlinux"
cp --reflink=auto "$ROOT_A/rootfs.ext4" "$BUNDLE_DIR/rootfs.ext4"
cp --reflink=auto "$ROOT_A/vmstate" "$BUNDLE_DIR/vmstate"
cp --reflink=auto "$ROOT_A/mem" "$BUNDLE_DIR/mem"
chown -R "$(id -u):$(id -g)" "$BUNDLE_DIR"

echo "==> bundle baked: $BUNDLE_DIR"
ls -lh "$BUNDLE_DIR"
