#!/usr/bin/env bash
# Offline A1 smoke (no k3s CRI switch, no Firecracker):
#   ptp+host-local → oas-vm-net (tap + G↔P NAT)
#   TCP: host → P:10000 --DNAT--> G:10000 (G simulated on lo)
set -euo pipefail
ROOT="$(cd "$(dirname "$0")" && pwd)"
CNI_BIN="${CNI_BIN:-/opt/cni/bin}"
CONFLIST="${CONFLIST:-$ROOT/conflist/10-oas-lab.conflist}"
SID="smoke$$"
NS="oas-${SID}"
NETNS="/var/run/netns/${NS}"

cleanup() {
  kill "${LISTEN_PID:-}" 2>/dev/null || true
  "$CNI_BIN/oas-cni-invoke" del \
    --config "$CONFLIST" --netns "$NETNS" --id "$SID" \
    --cni-path "$CNI_BIN" 2>/dev/null || true
  ip netns del "$NS" 2>/dev/null || true
}
trap cleanup EXIT

echo "== install plugins =="
bash "$ROOT/install.sh" "$CNI_BIN"

echo "== create netns $NS =="
ip netns add "$NS"

echo "== CNI ADD =="
RESULT=$("$CNI_BIN/oas-cni-invoke" add \
  --config "$CONFLIST" --netns "$NETNS" --id "$SID" \
  --cni-path "$CNI_BIN")
echo "$RESULT"
POD_IP=$(python3 -c 'import json,sys; d=json.loads(sys.argv[1]); print(d["ips"][0]["address"].split("/")[0])' "$RESULT")
echo "POD_IP=$POD_IP"

echo "== check tap + nft (DNAT P→G + SNAT→T on tap) =="
ip netns exec "$NS" ip addr show tapH0 | grep -q 172.16.0.1
ip netns exec "$NS" nft list table ip oas_vm | grep -q "dnat to 172.16.0.2"
ip netns exec "$NS" nft list table ip oas_vm | grep -q "snat to 172.16.0.1"

echo "== TCP DNAT host -> ${POD_IP}:10000 -> 172.16.0.2:10000 =="
# Simulate guest address on lo (no Firecracker required for this smoke).
ip netns exec "$NS" ip addr add 172.16.0.2/30 dev lo 2>/dev/null || true
ip netns exec "$NS" python3 -c '
import socket
s=socket.socket(); s.setsockopt(socket.SOL_SOCKET,socket.SO_REUSEADDR,1)
s.bind(("172.16.0.2",10000)); s.listen(1)
c,_=s.accept(); data=c.recv(64); c.sendall(b"pong"); c.close()
assert data.startswith(b"ping"), data
' &
LISTEN_PID=$!
sleep 0.3
REPLY=$(echo ping | timeout 3 nc -w 2 "$POD_IP" 10000)
[[ "$REPLY" == "pong" ]] || { echo "unexpected reply: $REPLY"; exit 1; }
wait "$LISTEN_PID"

echo "== OK: lab A1 (ptp + oas-vm-net + DNAT :10000) =="
