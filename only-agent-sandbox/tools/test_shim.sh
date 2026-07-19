#!/usr/bin/env bash
# Shim 单独测试：绕过 runtime，直接驱动 shim 二进制完成一次 snapshot 恢复 + 验证。
#
# 前置：bundle 已烘焙（`tools/bake_bundle.sh <bundle>`），产物在
#   $ARTIFACTS/snapshots/<bundle>/{vmlinux,rootfs.ext4,vmstate,mem}（含 virtio-net net1→tapH0）。
#
# 流程：
#   1. 建 netns `oas-<sid>` + tapH0 + 网关 IP（镜像 oas-net::setup）
#   2. 制备 per-VM rw ext4（镜像 oas-storage::provision）
#   3. 后台起 shim：`oas-runtime shim --config ... --sandbox-id <sid> --socket <uds> --log-file <f>`
#   4. 等 shim socket → `shim_smoke create` 触发恢复
#   5. 验证：firecracker 进程出现 + `shim_smoke state` = Running + `ip netns exec ... ssh`
#   6. （默认）`shim_smoke stop` 清理；`TEST_KEEP=1` 保留现场
#
# 用法：sudo TEST_KEEP=1 ./tools/test_shim.sh base-1 [sid]
# 需 root（jailer / netns / mkfs）。
set -euo pipefail

BUNDLE="${1:?usage: test_shim.sh <bundle> [sid]}"
SID="${2:-smoke-$$}"
KEEP="${TEST_KEEP:-0}"

# 路径（与 oas-config::Config::default() 一致；可由环境覆盖）
ARTIFACTS="${ARTIFACTS:-/var/lib/oas/artifacts}"
CHROOT_BASE="${CHROOT_BASE:-/home/yunfei/Code/Only-AgentSandbox/tmp/oas-test}"
RW_BASE="${RW_BASE:-/var/lib/oas/rw}"
RUN_BASE="${RUN_BASE:-/run/oas}"
LOG_DIR="${LOG_DIR:-/var/log/oas}"
CFG="${CFG:-/etc/oas/config.toml}"   # 不存在 → shim Config::load 回落 Default
GUEST_IP="${GUEST_IP:-172.16.0.2}"
TAP="${TAP:-tapH0}"
GATEWAY="${GATEWAY:-172.16.0.1}"
PREFIX="${PREFIX:-30}"

BIN="$(cd "$(dirname "$0")/.." && pwd)/target/debug/oas-runtime"
SMOKE="$(cd "$(dirname "$0")/.." && pwd)/target/debug/examples/shim_smoke"
BUNDLE_DIR="$ARTIFACTS/snapshots/$BUNDLE"
NS="oas-$SID"
SOCK="$RUN_BASE/oas-shim-$SID.sock"
RW="$RW_BASE/$SID.ext4"
LOG="$LOG_DIR/oas-shim-$SID.log"
SANDBOX_DIR="$CHROOT_BASE/firecracker/$SID"

[ -x "$BIN" ] || { echo "missing $BIN (cargo build first)"; exit 1; }
[ -x "$SMOKE" ] || { echo "missing $SMOKE (cargo build --example shim_smoke first)"; exit 1; }
[ -f "$BUNDLE_DIR/vmstate" ] || { echo "bundle not baked: $BUNDLE_DIR (run bake_bundle.sh $BUNDLE)"; exit 1; }

echo "==> shim smoke: bundle=$BUNDLE sid=$SID netns=$NS"
mkdir -p "$RUN_BASE" "$LOG_DIR" "$RW_BASE" "$CHROOT_BASE/firecracker"

cleanup() {
    set +e
    if [ "$KEEP" = "1" ]; then echo "[keep] 现场保留：netns=$NS sock=$SOCK jail=$SANDBOX_DIR"; exit 0; fi
    "$SMOKE" stop "$SOCK" "$SID" 2>/dev/null || true
    sleep 0.3
    ip netns del "$NS" 2>/dev/null || true
    rm -f "$SOCK" "$RW"
    rm -rf "$SANDBOX_DIR"
}
trap cleanup EXIT

# 1. netns + tapH0 + 网关（镜像 oas-net）
ip netns add "$NS" 2>/dev/null || true
ip netns exec "$NS" ip tuntap add dev "$TAP" mode tap
ip netns exec "$NS" ip addr add "$GATEWAY/$PREFIX" dev "$TAP"
ip netns exec "$NS" ip link set "$TAP" up

# 2. per-VM rw ext4（镜像 oas-storage）
rm -f "$RW"
truncate -s 1G "$RW"
mkfs.ext4 -F "$RW" >/dev/null

# 3. 起 shim（setsid 脱离，后台）
rm -f "$SOCK"
setsid "$BIN" shim --config "$CFG" --sandbox-id "$SID" --socket "$SOCK" --log-file "$LOG" &
SHIM_PID=$!
echo "    shim pid=$SHIM_PID, log= $LOG"

# 4. 等 shim socket
for _ in $(seq 1 100); do [ -S "$SOCK" ] && break; sleep 0.1; done
[ -S "$SOCK" ] || { echo "FAIL: shim socket 未出现，看 $LOG"; cat "$LOG" 2>/dev/null | tail -30; exit 1; }

# 5. 触发恢复
echo "==> shim_smoke create"
"$SMOKE" create "$SOCK" "$CFG" "$SID" "$BUNDLE_DIR" "$RW"

# 6. 验证 firecracker 进程
sleep 0.5
if pgrep -a firecracker >/dev/null; then
    echo "    OK: firecracker 进程存在"
else
    echo "FAIL: 未找到 firecracker 进程"; cat "$LOG" 2>/dev/null | tail -40; exit 1
fi

# 7. state = Running
echo "==> shim_smoke state"
STATE=$("$SMOKE" state "$SOCK" "$SID" | sed 's/state -> //')
echo "    state=$STATE"
[ "$STATE" = "Running" ] || { echo "FAIL: state != Running"; exit 1; }

# 8. SSH（可选；镜像需 sshd + 静态 IP 172.16.0.2）
echo "==> ssh 验证（ip netns exec $NS ssh $GUEST_IP）"
if ip netns exec "$NS" ssh -i /home/yunfei/workspace/bin/usefull_sh/ubuntu-.id_rsa -o StrictHostKeyChecking=no -o ConnectTimeout=5 -o BatchMode=yes root@"$GUEST_IP" "echo SSH_OK; uname -a" 2>/dev/null; then
    echo "    OK: SSH 通"
else
    echo "    WARN: SSH 不通（镜像 sshd/静态IP/密钥问题，非恢复链路问题）"
fi

echo "==> PASS: shim 单独恢复链路通过"
