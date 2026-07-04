source "$(dirname "$0")/env.sh"

# 仅本地静态 Pod 模式，不连 api-server。端口避开宿主真实 kubelet（10250/10248）。
kubelet \
  --container-runtime-endpoint="$RT_SOCK" \
  --image-service-endpoint="$RT_SOCK" \
  --pod-manifest-path="$MANIFEST_DIR" \
  --root-dir="$WORK/kubelet-root" \
  --fail-swap-on=false \
  --cgroup-driver=systemd \
  --runtime-request-timeout=30s \
  --port=15250 \
  --healthz-port=15248 \
  --read-only-port=0 \
  -v=4 \
  > "$LOG_DIR/kubelet.log" 2>&1 &
echo $! > "$WORK/kubelet.pid"
echo "kubelet started, pid=$(cat $WORK/kubelet.pid), log=$LOG_DIR/kubelet.log"