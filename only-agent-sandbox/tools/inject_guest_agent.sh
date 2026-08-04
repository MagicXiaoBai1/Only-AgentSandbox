#!/usr/bin/env bash
# 把 guest-agent 二进制注入 rootfs.ext4，并安装开机自启（systemd 或 rc.local）。
#
# 用法:
#   inject_guest_agent.sh <rootfs.ext4> <guest_agent_bin> [port]
#
# 环境变量:
#   MOUNT_DIR  临时挂载点（默认 /tmp/oas-rootfs-mnt-$$）
set -euo pipefail

ROOTFS="${1:?usage: inject_guest_agent.sh <rootfs.ext4> <guest_agent_bin> [port]}"
AGENT_BIN="${2:?usage: inject_guest_agent.sh <rootfs.ext4> <guest_agent_bin> [port]}"
PORT="${3:-10000}"
MOUNT_DIR="${MOUNT_DIR:-/tmp/oas-rootfs-mnt-$$}"

if [[ ! -f "$ROOTFS" ]]; then
  echo "rootfs not found: $ROOTFS" >&2
  exit 1
fi
if [[ ! -f "$AGENT_BIN" ]]; then
  echo "guest-agent binary not found: $AGENT_BIN" >&2
  exit 1
fi

cleanup() {
  set +e
  if mountpoint -q "$MOUNT_DIR" 2>/dev/null; then
    umount "$MOUNT_DIR"
  fi
  rmdir "$MOUNT_DIR" 2>/dev/null || true
}
trap cleanup EXIT

mkdir -p "$MOUNT_DIR"
mount -o loop "$ROOTFS" "$MOUNT_DIR"

install -d -m 0755 "$MOUNT_DIR/opt"
install -m 0755 "$AGENT_BIN" "$MOUNT_DIR/opt/guest_agent"

# 出向 DNS：静态镜像通常无 DHCP，写入公共解析器（经 guest egress NAT 可达）。
mkdir -p "$MOUNT_DIR/etc"
if [[ ! -f "$MOUNT_DIR/etc/resolv.conf" ]] || ! grep -q 'nameserver' "$MOUNT_DIR/etc/resolv.conf" 2>/dev/null; then
  cat >"$MOUNT_DIR/etc/resolv.conf" <<'EOF'
nameserver 8.8.8.8
nameserver 1.1.1.1
EOF
  echo "wrote /etc/resolv.conf (8.8.8.8 / 1.1.1.1)"
fi

if [[ -d "$MOUNT_DIR/etc/systemd/system" ]]; then
  cat >"$MOUNT_DIR/etc/systemd/system/guest-agent.service" <<EOF
[Unit]
Description=Only AgentSandbox guest-agent
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart=/opt/guest_agent --port ${PORT}
Restart=always
RestartSec=1
User=root

[Install]
WantedBy=multi-user.target
EOF
  mkdir -p "$MOUNT_DIR/etc/systemd/system/multi-user.target.wants"
  ln -sfn ../guest-agent.service \
    "$MOUNT_DIR/etc/systemd/system/multi-user.target.wants/guest-agent.service"
  echo "installed systemd unit guest-agent.service (port=${PORT})"
else
  mkdir -p "$MOUNT_DIR/etc"
  if [[ -f "$MOUNT_DIR/etc/rc.local" ]]; then
    grep -q '/opt/guest_agent' "$MOUNT_DIR/etc/rc.local" || \
      sed -i "\$i /opt/guest_agent --port ${PORT} \&" "$MOUNT_DIR/etc/rc.local"
  else
    cat >"$MOUNT_DIR/etc/rc.local" <<EOF
#!/bin/sh
/opt/guest_agent --port ${PORT} &
exit 0
EOF
    chmod +x "$MOUNT_DIR/etc/rc.local"
  fi
  echo "installed rc.local autostart (port=${PORT})"
fi

sync
echo "guest-agent injected into $ROOTFS"
