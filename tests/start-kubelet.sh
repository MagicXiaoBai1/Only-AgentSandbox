source "$(dirname "$0")/env.sh"

# 仅本地静态 Pod 模式，不连 api-server。端口避开宿主真实 kubelet（10250/10248）。
# /home/yunfei/Code/Only-AgentSandbox/tests/kubelet \
kubelet \
  --container-runtime-endpoint="$RT_SOCK" \
  --image-service-endpoint="$RT_SOCK" \
  --pod-manifest-path="$MANIFEST_DIR" \
  --root-dir="$WORK/kubelet-root" \
  --fail-swap-on=false \
  --cgroup-driver=systemd \
  --cgroups-per-qos=false \
  --enforce-node-allocatable="" \
  --runtime-request-timeout=30s \
  --sync-frequency=10s \
  --port=15250 \
  --healthz-port=15248 \
  --read-only-port=0 \
  -v=5 \
  --vmodule=kuberuntime_manager=6,kuberuntime_container=6,kuberuntime_sandbox=6,util=6,labels=6,pod_workers=5,generic=5 \
  > "$LOG_DIR/kubelet.log" 2>&1 &
echo $! > "$WORK/kubelet.pid"
echo "kubelet started, pid=$(cat $WORK/kubelet.pid), log=$LOG_DIR/kubelet.log"
