#!/usr/bin/env bash
# 只读挂载 VM rootfs，检查 sshd / 网络 / init 系统状态，决定怎么配公钥与静态 IP。
# 用法：sudo ./tools/vm_inspect.sh [rootfs]
#   默认 rootfs = /home/yunfei/workspace/snap_double_shot/vm_resourse/rootfs.ext4
set -euo pipefail

ROOTFS="${1:-/home/yunfei/workspace/snap_double_shot/vm_resourse/rootfs.ext4}"
MNT="$(mktemp -d /tmp/oas-rootfs.XXXXXX)"

cleanup() {
    sync; mountpoint -q "$MNT" && umount -R "$MNT" 2>/dev/null || true
    rmdir "$MNT" 2>/dev/null || true
}
trap cleanup EXIT

echo "==> 挂载 $ROOTFS -> $MNT（只读）"
mount -o ro,loop "$ROOTFS" "$MNT"

echo "=== /etc/os-release ==="; cat "$MNT/etc/os-release" 2>/dev/null || echo "（无）"
echo "=== init 系统 ==="
for p in /sbin/init /lib/systemd/systemd /usr/lib/systemd/systemd /sbin/openrc-init; do
    [ -e "$MNT$p" ] && echo "  有 $p"
done
echo "  systemctl? $([ -x "$MNT/bin/systemctl" ] && echo yes || echo no)"
echo "  /etc/inittab? $([ -f "$MNT/etc/inittab" ] && echo yes || echo no)"
echo "  /etc/init.d/rcS? $([ -f "$MNT/etc/init.d/rcS" ] && echo yes || echo no)"

echo "=== sshd 是否安装 ==="
ls -l "$MNT/usr/sbin/sshd" "$MNT/usr/lib/systemd/system/ssh.service" "$MNT/etc/init.d/ssh" "$MNT/etc/init.d/sshd" 2>/dev/null || echo "  （未找到 sshd）"

echo "=== /etc/ssh/sshd_config 关键项 ==="
grep -iE '^(PermitRootLogin|PubkeyAuthentication|PasswordAuthentication|AuthorizedKeysFile|Port|ListenAddress)' "$MNT/etc/ssh/sshd_config" 2>/dev/null || echo "  （无 sshd_config）"

echo "=== 现有 authorized_keys ==="
find "$MNT/root" "$MNT/home" -name authorized_keys 2>/dev/null | head || echo "  （无）"

echo "=== 网络配置方式 ==="
echo "--- /etc/network/interfaces ---"; cat "$MNT/etc/network/interfaces" 2>/dev/null || echo "  （无）"
echo "--- /etc/systemd/network/ ---"; ls "$MNT/etc/systemd/network/" 2>/dev/null || echo "  （无）"
echo "--- /etc/netplan/ ---"; ls "$MNT/etc/netplan/" 2>/dev/null || echo "  （无）"
echo "--- /etc/rc.local ---"; cat "$MNT/etc/rc.local" 2>/dev/null | head -20 || echo "  （无）"
echo "--- /etc/init.d 里网络相关 ---"; ls "$MNT/etc/init.d/" 2>/dev/null | grep -iE 'net|eth|interface' || echo "  （无）"

echo "=== /root/.ssh ? ==="; ls -la "$MNT/root/.ssh/" 2>/dev/null || echo "  （无 /root/.ssh）"
echo "=== 启动脚本线索（看 eth0/172.16 配置在哪）==="
grep -rIl '172.16.0.2\|eth0\|ip addr add\|ifconfig' "$MNT/etc" 2>/dev/null | head -10 || echo "  （未搜到）"
