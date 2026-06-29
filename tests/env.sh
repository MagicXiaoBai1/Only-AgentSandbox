
export WORK="/home/yunfei/Code/Only-AgentSandbox/tests/agent-rt-test"
# export RT_SOCK="unix:///var/run/agent-runtime.sock"      # standalone kubelet 监听目录
export RT_SOCK="unix:///run/oas.sock"      # standalone kubelet 监听目录
export MANIFEST_DIR="$WORK/static-pods"                   # 待投放的 yaml 模板池（不被 kubelet 监听）
export POOL_DIR="$WORK/pod-pool"                          # pod-pool/ 下的模板拷贝至此
export LOG_DIR="$WORK/logs"
export RESULT_DIR="$WORK/results"
export EXT_TARGET_IP="10.0.0.250"                         # T2 外部靶机 IP（改为你自己的）
export EXT_TARGET_DOMAIN="registry.test.local"            # 固定域名（DNS 验证用）
export CRITCL="--runtime-endpoint $RT_SOCK --image-endpoint $RT_SOCK"
mkdir -p "$MANIFEST_DIR" "$POOL_DIR" "$LOG_DIR" "$RESULT_DIR"