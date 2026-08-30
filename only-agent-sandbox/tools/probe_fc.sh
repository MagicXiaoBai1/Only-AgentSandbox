#!/usr/bin/env bash
# 探测一个 TEST_KEEP=1 保留现留下来的 firecracker API socket。
# 用法：sudo ./tools/probe_fc.sh <sid>
#   先：sudo TEST_KEEP=1 ./tools/test_shim.sh base-1 <sid>
#   再：sudo ./tools/probe_fc.sh <sid>
#
# 回答两个问题：
#   1. firecracker 进程是否还在？
#   2. API socket 是否响应？响应什么？是否会主动关闭连接（keep-alive）？
set -euo pipefail

SID="${1:?usage: probe_fc.sh <sid>}"
CHROOT_BASE="${CHROOT_BASE:-/home/yunfei/Code/Only-AgentSandbox/tmp/oas-test}"
JAIL_ROOT="$CHROOT_BASE/firecracker/$SID"
SOCK="$JAIL_ROOT/run/firecracker.socket"

echo "==> sid=$SID"
echo "==> jail_root=$JAIL_ROOT"
echo "==> sock=$SOCK"

echo "=== firecracker 进程（按 jail root 命中）==="
found=0
for pid in $(pgrep -x firecracker 2>/dev/null || true); do
    root=$(readlink "/proc/$pid/root" 2>/dev/null || true)
    if [ "$root" = "$JAIL_ROOT" ]; then
        echo "  pid=$pid root=$root"
        found=1
    fi
done
[ "$found" = 1 ] || echo "  未找到命中 $JAIL_ROOT 的 firecracker"
echo "  （所有 firecracker 进程：）"
pgrep -af firecracker 2>/dev/null | sed 's/^/    /' || echo "    （无）"

echo "=== socket 文件 ==="
ls -l "$SOCK" 2>&1 || true

echo "=== firecracker 自身日志（jail_root/fc-<sid>.log）==="
FC_LOG="$JAIL_ROOT/fc-$SID.log"
if [ -f "$FC_LOG" ]; then cat "$FC_LOG"; else echo "  无 fc 日志（$FC_LOG 不存在）"; fi

echo "=== GET / 探测（Connection: close，3s 超时）==="
python3 - "$SOCK" <<'PY'
import socket, sys, time
sock = sys.argv[1]
try:
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    s.settimeout(3)
    s.connect(sock)
    req = b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
    s.sendall(req)
    chunks = []
    start = time.time()
    while True:
        try:
            data = s.recv(4096)
        except socket.timeout:
            print(f"[recv 超时 {time.time()-start:.1f}s，已收到 {sum(len(c) for c in chunks)} 字节]")
            break
        if not data:
            print("[对端关闭连接 EOF]")
            break
        chunks.append(data)
        if time.time() - start > 3:
            print("[超过 3s，停止读取]")
            break
    raw = b"".join(chunks)
    print("--- 响应原始字节 ---")
    print(repr(raw[:800]))
    print("--- 解码预览 ---")
    print(raw[:800].decode("utf-8", "replace"))
except Exception as e:
    print(f"[异常: {type(e).__name__}: {e}]")
PY
