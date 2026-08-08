#!/usr/bin/env bash
# Standalone FC + CNI A1 smoke via crictl (NO kubelet, NO k3s CRI switch).
#
# Context after 联调 §9 rollback:
#   - k3s CRI = containerd (not OAS)
#   - standalone kubelet (tests/start-kubelet.sh) is optional and usually OFF
#   - /run/oas.sock may still be an unrelated old OAS — this script NEVER uses it
#
# Uses: /run/oas-calico.sock + config-cni-lab.toml + Calico-tree oas-runtime only.
#
#   sudo env PATH=$PATH ./cni/smoke_fc_cni.sh
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CNI_ROOT="$(cd "$(dirname "$0")" && pwd)"
CFG="$CNI_ROOT/config-cni-lab.toml"
SOCK="/run/oas-calico.sock"
POD_JSON="$CNI_ROOT/fixtures/podsandbox.json"
RT="unix://${SOCK}"
SID_GLOB='sb-000000000000000*'   # store starts at 1 after wipe

log() { echo "$*"; }

cleanup_calico_orphans() {
  set +e
  # Only Calico-track (never /run/oas.sock / ck OAS / unrelated FC).
  pkill -f 'config-cni-lab.toml' 2>/dev/null || true
  pkill -f 'oas-shim-sb-00000000000000' 2>/dev/null || true
  # FC still jailed under oas-test-calico
  for pid in $(pgrep -x firecracker 2>/dev/null || true); do
    if ls -l "/proc/$pid/root" 2>/dev/null | grep -q 'oas-test-calico'; then
      kill -9 "$pid" 2>/dev/null || true
    fi
  done
  sleep 0.5
  for ns in /var/run/netns/oas-sb-*; do
    [[ -e "$ns" ]] || continue
    ip netns del "$(basename "$ns")" 2>/dev/null || true
  done
  rm -f /run/oas-calico/oas-shim-sb-*.sock "$SOCK"
  rm -rf /home/yunfei/oas-test-calico/firecracker/sb-*
  rm -f /var/lib/oas/store-calico.redb /var/lib/oas/store-calico.redb-*
  rm -f /var/lib/oas/rw-calico/sb-*.ext4
  set -e
}

KEEP="${KEEP:-0}"
cleanup() {
  set +e
  if [[ "$KEEP" == "1" ]]; then
    echo "[KEEP=1] leaving OAS pid=${OAS_PID:-} pod=${POD_ID:-} sock=$SOCK"
    exit 0
  fi
  if [[ -n "${POD_ID:-}" ]]; then
    crictl --runtime-endpoint "$RT" stopp "$POD_ID" 2>/dev/null || true
    crictl --runtime-endpoint "$RT" rmp -f "$POD_ID" 2>/dev/null || true
  fi
  if [[ -n "${OAS_PID:-}" ]]; then
    kill "$OAS_PID" 2>/dev/null || true
    wait "$OAS_PID" 2>/dev/null || true
  fi
  cleanup_calico_orphans
}
trap cleanup EXIT

log "== pre-clean Calico orphans (scoped) =="
cleanup_calico_orphans

# Dedicated target dir so we never pick up stale rsync'd ./target without CNI.
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target-calico}"
log "== cargo build -p oas-runtime (CARGO_TARGET_DIR=$CARGO_TARGET_DIR) =="
(cd "$ROOT" && cargo build -p oas-runtime 2>&1 | tail -5)
BIN="$CARGO_TARGET_DIR/debug/oas-runtime"
[[ -x "$BIN" ]] || { echo "missing $BIN"; exit 1; }
if ! grep -aob 'cni ADD failed' "$BIN" >/dev/null; then
  echo "BIN lacks CNI support: $BIN"
  ls -la "$BIN"
  exit 1
fi
log "BIN=$BIN"

bash "$CNI_ROOT/install.sh" /opt/cni/bin
mkdir -p /run/oas-calico /var/log/oas-calico /var/lib/oas/rw-calico /home/yunfei/oas-test-calico
: >"$CNI_ROOT/oas-calico.runtime.log"

log "== start Calico OAS on $SOCK (not /run/oas.sock) =="
setsid env OAS_SOCKET="$SOCK" "$BIN" --config "$CFG" \
  >>"$CNI_ROOT/oas-calico.runtime.log" 2>&1 &
OAS_PID=$!
log "oas-runtime pid=$OAS_PID"

for _ in $(seq 1 50); do
  [[ -S "$SOCK" ]] && crictl --runtime-endpoint "$RT" info >/dev/null 2>&1 && break
  sleep 0.2
done
crictl --runtime-endpoint "$RT" info | head -5

log "== RunPodSandbox (may take minutes: copy mem + snapshot load) =="
START=$(date +%s)
POD_ID=$(crictl --runtime-endpoint "$RT" runp "$POD_JSON")
log "POD_ID=$POD_ID elapsed=$(( $(date +%s) - START ))s"

POD_IP=""
for _ in $(seq 1 30); do
  ST=$(crictl --runtime-endpoint "$RT" inspectp "$POD_ID" 2>/dev/null || true)
  POD_IP=$(python3 -c 'import json,sys; d=json.loads(sys.stdin.read() or "{}"); print((((d.get("status") or {}).get("network") or {}).get("ip") or ""))' <<<"$ST")
  [[ -n "$POD_IP" ]] && break
  sleep 0.5
done
[[ -n "$POD_IP" ]] || { echo "no pod IP"; tail -80 "$CNI_ROOT/oas-calico.runtime.log"; exit 1; }
log "POD_IP=$POD_IP"
crictl --runtime-endpoint "$RT" pods

NS="oas-${POD_ID}"
[[ -e "/var/run/netns/$NS" ]] || { echo "missing netns $NS"; exit 1; }

log "== wait shim State=Running =="
SHIM_SOCK="/run/oas-calico/oas-shim-${POD_ID}.sock"
for i in $(seq 1 120); do
  # State via strings in shim log is hard; probe guest instead once FC is up.
  if ip netns exec "$NS" timeout 1 bash -c "echo >/dev/tcp/172.16.0.2/10000" 2>/dev/null; then
    log "guest 172.16.0.2:10000 open (after ${i}s)"
    break
  fi
  if [[ $i -eq 120 ]]; then
    log "FAIL: guest not listening after 120s"
    log "--- shim log (info) ---"
    grep -E 'INFO|ERROR|create failed|Running|Pending' "/var/log/oas-calico/oas-shim-${POD_ID}.log" 2>/dev/null | tail -40
    log "--- fc processes ---"
    pgrep -af firecracker | head -10
    exit 1
  fi
  sleep 1
done

log "== check tap + nft =="
ip netns exec "$NS" ip addr show tapH0 | grep -q 172.16.0.1
ip netns exec "$NS" nft list table ip oas_vm | grep -q "dnat to 172.16.0.2"

log "== dial PodIP:10000 =="
ok=0
for _ in $(seq 1 20); do
  if timeout 2 bash -c "echo >/dev/tcp/${POD_IP}/10000" 2>/dev/null; then ok=1; break; fi
  sleep 0.5
done
[[ "$ok" == 1 ]] || { echo "FAIL: cannot connect ${POD_IP}:10000 (guest OK but DNAT?)"; exit 1; }

log "== OK: CNI A1 dial ${POD_IP}:10000 (crictl-direct, no kubelet) =="
