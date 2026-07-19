#!/usr/bin/env bash
# 深入检查一个还活着的 firecracker 进程：它到底 chroot/挂载在哪、socket/log 真实路径、fd。
# 用法：sudo ./tools/inspect_fc.sh <fc_pid>
#   先 sudo TEST_KEEP=1 ./tools/test_shim.sh base-1 probe-3
#   再 pgrep -af firecracker | grep probe-3   # 拿到 pid
#   再 sudo ./tools/inspect_fc.sh <pid>
set -euo pipefail

PID="${1:?usage: inspect_fc.sh <fc_pid>}"
JAIL_ROOT_GUESS="${JAIL_ROOT:-/home/yunfei/Code/Only-AgentSandbox/tmp/oas-test/firecracker/probe-3}"

echo "==> pid=$PID"
echo "=== cmdline ==="; tr '\0' ' ' < /proc/$PID/cmdline 2>&1; echo
echo "=== /proc/$PID/root （firecracker 的真实根）==="; readlink /proc/$PID/root 2>&1
echo "=== /proc/$PID/cwd ==="; readlink /proc/$PID/cwd 2>&1
echo "=== status (state + NSpid) ==="; grep -E '^(State|NSpid|PPid|Uid|Gid):' /proc/$PID/status 2>&1
echo "=== namespace inodes ==="
for ns in pid mnt net user; do
    printf "  %-4s " "$ns"; readlink /proc/$PID/ns/$ns 2>&1
done
echo "  (本 shell) mnt: $(readlink /proc/self/ns/mnt 2>&1)  pid: $(readlink /proc/self/ns/pid 2>&1)"

echo "=== firecracker 视角下的 /run 与日志 ==="
ls -la /proc/$PID/root/run/ 2>&1 | head -20
echo "--- /proc/$PID/root/fc-*.log ---"; ls -la /proc/$PID/root/fc-*.log 2>&1 || true
echo "--- 日志内容（firecracker 自己写的）---"
for f in /proc/$PID/root/fc-*.log; do [ -f "$f" ] && { echo ">>> $f"; tail -50 "$f"; }; done 2>&1

echo "=== firecracker 打开的 fd（找 socket 与日志的真实宿主路径）==="
ls -la /proc/$PID/fd/ 2>&1 | head -40

echo "=== 该 pid 是否在监听 unix socket（ss）==="
ss -xlp 2>/dev/null | grep -i firecracker || echo "  (ss 未发现 firecracker socket)"

echo "=== 对照：宿主侧 jail_root 实际内容 ==="
ls -la "$JAIL_ROOT_GUESS" 2>&1 | head -30
