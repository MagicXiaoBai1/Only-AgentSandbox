source "$(dirname "$0")/env.sh"

# 仅本地静态 Pod 模式，不连 api-server
kubelet \
  --container-runtime-endpoint="$RT_SOCK" \
  --image-service-endpoint="$RT_SOCK" \
  --pod-manifest-path="$MANIFEST_DIR" \
  --root-dir="$WORK/kubelet-root" \
  --fail-swap-on=false \
  --cgroup-driver=systemd \
  --runtime-request-timeout=30s \
  -v=4 \
  > "$LOG_DIR/kubelet.log" 2>&1 &
echo $! > "$WORK/kubelet.pid"
echo "kubelet started, pid=$(cat $WORK/kubelet.pid), log=$LOG_DIR/kubelet.log"