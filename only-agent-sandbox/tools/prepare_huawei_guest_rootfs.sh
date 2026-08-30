#!/usr/bin/env bash
# Prepare a *copy* of a bootable ext4 rootfs for the Huawei phase-2 guest-agent.
#
# The phase-2 guest-agent is normally packaged as an OCI image.  Firecracker
# snapshot restore, however, boots an ext4 filesystem directly; an OCI image is
# not a bootable rootfs.  This script installs the already-built guest_agent
# binary and a systemd unit into a copied ext4 image, without changing the base
# image supplied by the caller.
#
# Usage (must run as root because it loop-mounts an ext4 image):
#   sudo ./tools/prepare_huawei_guest_rootfs.sh \
#       <base-rootfs.ext4> <guest_agent-static-bin> <output-rootfs.ext4>
#
# Optional environment:
#   GUEST_IP=172.16.0.2 GUEST_PREFIX=30 GUEST_GATEWAY=172.16.0.1
#   WRITE_STATIC_NETWORK=1       # write a systemd-networkd eth0 config
#   GUEST_AGENT_PORT=10000
set -euo pipefail

BASE_ROOTFS="${1:?usage: $0 <base-rootfs.ext4> <guest_agent-bin> <output-rootfs.ext4>}"
GUEST_AGENT_BIN="${2:?usage: $0 <base-rootfs.ext4> <guest_agent-bin> <output-rootfs.ext4>}"
OUT_ROOTFS="${3:?usage: $0 <base-rootfs.ext4> <guest_agent-bin> <output-rootfs.ext4>}"
GUEST_IP="${GUEST_IP:-172.16.0.2}"
GUEST_PREFIX="${GUEST_PREFIX:-30}"
GUEST_GATEWAY="${GUEST_GATEWAY:-172.16.0.1}"
GUEST_AGENT_PORT="${GUEST_AGENT_PORT:-10000}"
WRITE_STATIC_NETWORK="${WRITE_STATIC_NETWORK:-0}"

[[ $(id -u) -eq 0 ]] || { echo "must run as root" >&2; exit 1; }
[[ -f "$BASE_ROOTFS" ]] || { echo "base rootfs not found: $BASE_ROOTFS" >&2; exit 1; }
[[ -f "$GUEST_AGENT_BIN" && -x "$GUEST_AGENT_BIN" ]] || {
    echo "guest-agent binary must exist and be executable: $GUEST_AGENT_BIN" >&2; exit 1;
}
[[ "$BASE_ROOTFS" != "$OUT_ROOTFS" ]] || { echo "output rootfs must differ from base rootfs" >&2; exit 1; }
[[ ! -e "$OUT_ROOTFS" ]] || { echo "refusing to overwrite output rootfs: $OUT_ROOTFS" >&2; exit 1; }
[[ "$GUEST_PREFIX" =~ ^([0-9]|[12][0-9]|3[0-2])$ ]] || { echo "invalid GUEST_PREFIX" >&2; exit 1; }
[[ "$GUEST_AGENT_PORT" =~ ^[0-9]+$ ]] || { echo "invalid GUEST_AGENT_PORT" >&2; exit 1; }

OUT_PARENT=$(dirname "$OUT_ROOTFS")
mkdir -p "$OUT_PARENT"
MOUNT_DIR=$(mktemp -d /tmp/oas-rootfs.XXXXXX)
MOUNTED=0
cleanup() {
    set +e
    if [[ "$MOUNTED" == 1 ]]; then umount "$MOUNT_DIR"; fi
    rmdir "$MOUNT_DIR"
}
trap cleanup EXIT

# Huawei rootfs 也属于 immutable 模板派生路径；要求内核 reflink 成功，
# 避免在不支持 CoW 的节点上产生看似成功但实际全量复制的结果。
cp --reflink=always "$BASE_ROOTFS" "$OUT_ROOTFS"
mount -o loop "$OUT_ROOTFS" "$MOUNT_DIR"
MOUNTED=1

[[ -x "$MOUNT_DIR/bin/bash" ]] || { echo "rootfs does not contain /bin/bash" >&2; exit 1; }
[[ -d "$MOUNT_DIR/etc/systemd/system" ]] || {
    echo "rootfs does not contain systemd unit directory; use a systemd-based bootable rootfs" >&2; exit 1;
}

install -D -m 0755 "$GUEST_AGENT_BIN" "$MOUNT_DIR/usr/local/bin/guest_agent"
install -d -m 0755 "$MOUNT_DIR/workspace"

cat >"$MOUNT_DIR/etc/systemd/system/oas-guest-agent.service" <<EOF
[Unit]
Description=Huawei Code Agent guest service
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart=/usr/local/bin/guest_agent --port ${GUEST_AGENT_PORT} --idle-timeout 0
Restart=always
RestartSec=1
WorkingDirectory=/workspace

[Install]
WantedBy=multi-user.target
EOF
install -d -m 0755 "$MOUNT_DIR/etc/systemd/system/multi-user.target.wants"
ln -sfn ../oas-guest-agent.service \
    "$MOUNT_DIR/etc/systemd/system/multi-user.target.wants/oas-guest-agent.service"

if [[ "$WRITE_STATIC_NETWORK" == 1 ]]; then
    install -d -m 0755 "$MOUNT_DIR/etc/systemd/network"
    cat >"$MOUNT_DIR/etc/systemd/network/10-oas-eth0.network" <<EOF
[Match]
Name=eth0

[Network]
Address=${GUEST_IP}/${GUEST_PREFIX}
Gateway=${GUEST_GATEWAY}
EOF
fi

sync
echo "prepared rootfs: $OUT_ROOTFS"
echo "guest agent: /usr/local/bin/guest_agent (${GUEST_AGENT_PORT}/tcp)"
echo "static network written: $WRITE_STATIC_NETWORK"
