#!/usr/bin/env bash
# oas-ctrd-shim 端到端测试：起一个隔离的 containerd 1.6.33 实例，用 ctr --rootfs 驱动
# `containerd-shim-oas-v2` 走完一条 task 的 create/start/list/kill/delete 全链路。
#
# 为什么隔离：本机系统 containerd 正在跑真实业务（moby + k8s.io），绝不能碰
# /run/containerd。本脚本起独立 containerd，socket/root/state 全落 /tmp 下，互不干扰。
#
# MockVm 边界（重要）：当前 shim 下层是 MockVm——不起真实进程，pid 是占位值 424242。
# 故本 e2e 只验证 containerd↔shim 的 ttrpc 协议链路与状态机，不验证真实 workload：
#   - 断言 task 进入 RUNNING 且 pid=424242（shim 把 VM pid 透传给 containerd）。
#   - 断言 kill+delete 后 shim 进程退出（退出门控）。
#   - 不要用 `ctr t exec`（NOT_FOUND）或 `ctr t wait`（会挂）；MockVm 无真实进程可 exec/wait。
#
# 用法：sudo ./tools/e2e-ctrd-shim.sh
#   需 sudo（containerd 要 root 起 netns/cgroup/socket）。
#   TEST_KEEP=1 sudo ./tools/e2e-ctrd-shim.sh   # 失败或结束时保留现场便于排查
set -euo pipefail

KEEP="${TEST_KEEP:-0}"

# 路径
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"          # only-agent-sandbox/
WORK="${WORK:-/tmp/oas-shim-e2e}"
SOCK="$WORK/run/containerd.sock"
CTRD_LOG="$WORK/logs/containerd.log"
CTRD_PID_FILE="$WORK/containerd.pid"
ROOTFS="$WORK/rootfs"
RUNTIME="io.containerd.oas.v2"
SHIM_BIN_NAME="containerd-shim-oas-v2"
TASK_ID="e2e-test"
NS="oas-e2e"

SHIM_BIN="$REPO_DIR/target/debug/$SHIM_BIN_NAME"
CTR="ctr -a $SOCK -n $NS"

# 颜色（非 tty 自动降级）
if [ -t 1 ]; then
    C_G=$'\033[32m'; C_R=$'\033[31m'; C_Y=$'\033[33m'; C_B=$'\033[34m'; C_0=$'\033[0m'
else
    C_G=''; C_R=''; C_Y=''; C_B=''; C_0=''
fi

ok()    { echo "    ${C_G}OK${C_0}: $*"; }
step()  { echo "${C_B}==>${C_0} $*"; }
die()   { echo "${C_R}FAIL${C_0}: $*"; exit 1; }

# ---- 清理 -------------------------------------------------------------------
cleanup() {
    set +e
    if [ "$KEEP" = "1" ]; then
        echo ""
        echo "${C_Y}[keep]${C_0} 现场保留："
        echo "  containerd sock : $SOCK"
        echo "  containerd log  : $CTRD_LOG"
        echo "  work dir        : $WORK"
        [ -f "$CTRD_PID_FILE" ] && echo "  containerd pid  : $(cat "$CTRD_PID_FILE")"
        exit 0
    fi
    # 停 containerd（会顺带收 shim）
    if [ -f "$CTRD_PID_FILE" ]; then
        sudo kill "$(cat "$CTRD_PID_FILE")" 2>/dev/null
        for _ in $(seq 1 20); do
            sudo kill -0 "$(cat "$CTRD_PID_FILE")" 2>/dev/null || break
            sleep 0.1
        done
        sudo kill -9 "$(cat "$CTRD_PID_FILE")" 2>/dev/null || true
    fi
    # 兜底杀残留 shim（仅匹配本测试的 namespace，绝不误伤系统 runc shim）
    sudo pkill -f "$SHIM_BIN_NAME.*$NS" 2>/dev/null || true
    sudo rm -rf "$WORK"
}
trap cleanup EXIT INT TERM

# ---- 0. 前置检查 ------------------------------------------------------------
step "前置检查"
command -v containerd >/dev/null || die "未找到 containerd，请先安装"
command -v ctr        >/dev/null || die "未找到 ctr"
[ -x "$SHIM_BIN" ] || {
    echo "    shim 未编译，执行 cargo build -p oas-ctrd-shim ..."
    (cd "$REPO_DIR" && cargo build -p oas-ctrd-shim) || die "shim 编译失败"
}
containerd --version | head -1
ok "shim 二进制: $SHIM_BIN"

# 确保是 root 执行（containerd 需要）
[ "$(id -u)" -eq 0 ] || die "请用 sudo 运行（containerd 需要 root）"

# ---- 1. 建隔离工作区 + 安装 shim -------------------------------------------
step "建隔离工作区: $WORK"
sudo rm -rf "$WORK"
mkdir -p "$WORK"/{root,state,run,opt/bin,logs,rootfs}
# rootfs 留空目录即可：MockVm 不读内容，containerd 只需 root.path 指向一个存在的目录。

# containerd 1.6.x 对 shim 二进制有属主/权限校验 → root 属主 0755 安装到 opt/bin。
sudo install -m 0755 -o root -g root "$SHIM_BIN" "$WORK/opt/bin/$SHIM_BIN_NAME"
ok "shim 已安装到 $WORK/opt/bin/$SHIM_BIN_NAME (root:root 0755)"

# ---- 2. 写最小 config.toml --------------------------------------------------
# 仅设隔离 root/state + opt 路径；runtime 靠 binary-name 约定自动发现，无需注册。
# 其余插件走 containerd 默认。
step "写 containerd config.toml"
cat > "$WORK/config.toml" <<EOF
root = "$WORK/root"
state = "$WORK/state"

# 只用 ctr（不走 kubelet/CRI），禁用 CRI 插件避免其初始化噪音/失败拖垮启动。
disabled_plugins = ["io.containerd.grpc.v1.cri"]

[plugins."io.containerd.internal.v1.opt"]
  path = "$WORK/opt"
EOF
ok "config: $WORK/config.toml"

# ---- 3. 起 containerd -------------------------------------------------------
step "起隔离 containerd"
# sudo 会重置 PATH，显式带上 shim 目录：opt/bin 解析失败时回落 PATH 也能找到 shim。
sudo env "PATH=$WORK/opt/bin:$PATH" containerd -c "$WORK/config.toml" --address "$SOCK" \
    > "$CTRD_LOG" 2>&1 &
CTRD_PID=$!
echo "$CTRD_PID" > "$CTRD_PID_FILE"
echo "    containerd pid=$CTRD_PID, log=$CTRD_LOG"

# 轮询等 socket 就绪
ready=0
for _ in $(seq 1 100); do
    if sudo $CTR version >/dev/null 2>&1; then ready=1; break; fi
    sleep 0.1
done
[ "$ready" = "1" ] || { echo "    containerd 启动失败，日志尾部:"; tail -40 "$CTRD_LOG"; die "containerd socket 未就绪"; }
ok "containerd 就绪: $SOCK"

# ---- 4. 创建并启动 task -----------------------------------------------------
step "ctr run --rootfs (runtime=$RUNTIME)"
# --rootfs  : 免镜像/免 registry，OCI spec root.path 指向空目录。
# --null-io : shim 不开 FIFO（MockVm 不 open io 端，否则会挂）。
# -d         : detach，不在 task.Wait 上挂（MockVm.wait 仅 stop 后返回）。
# /bin/true  : 仅填充 OCI spec process.args，MockVm 不会真正 exec。
sudo $CTR run -d --runtime "$RUNTIME" --rootfs "$ROOTFS" --null-io "$TASK_ID" /bin/true \
    || { echo "    ctr run 失败，containerd 日志尾部:"; tail -50 "$CTRD_LOG"; die "ctr run 失败"; }
ok "task 已创建并启动: $TASK_ID"

# 给 containerd 一点时间拉起 shim 并完成 create/start
for _ in $(seq 1 50); do
    sudo $CTR t list 2>/dev/null | grep -q "$TASK_ID" && break
    sleep 0.1
done

# ---- 5. 断言 RUNNING + pid=424242 + shim 进程在 -----------------------------
step "断言 task 状态"
list_out="$(sudo $CTR t list 2>/dev/null || true)"
echo "$list_out" | sed 's/^/    | /'

echo "$list_out" | grep -q "$TASK_ID" || die "task 未出现在 ctr t list"
echo "$list_out" | grep -q "RUNNING"  || die "task 未进入 RUNNING（MockVm 应置 Running）"
echo "$list_out" | grep -q "424242"   || die "task pid != 424242（MockVm 占位 pid 未透传）"
ok "task=$TASK_ID state=RUNNING pid=424242"

# shim 常驻进程应存在（仅匹配本 namespace，不误伤系统）
shim_cnt="$(sudo pgrep -f "$SHIM_BIN_NAME.*$NS" | wc -l)"
[ "$shim_cnt" -ge 1 ] || die "未找到 shim 进程（pgrep $SHIM_BIN_NAME.*$NS）"
ok "shim 常驻进程存在: $shim_cnt 个"

# ---- 6. kill + delete -------------------------------------------------------
step "ctr t kill + delete"
sudo $CTR t kill "$TASK_ID" || die "ctr t kill 失败"
ok "kill 完成（MockVm.stop → wait 返回 0）"

# delete 可能需要 task 先 stop；kill 后应可删，必要时 --force
sudo $CTR t delete "$TASK_ID" 2>/dev/null || sudo $CTR t delete --force "$TASK_ID" \
    || { echo "    containerd 日志尾部:"; tail -40 "$CTRD_LOG"; die "ctr t delete 失败"; }
ok "delete 完成（注册表清空 → exit.signal）"

# ---- 7. 断言 task 清空 + shim 已退出 ----------------------------------------
step "断言清理结果"
for _ in $(seq 1 50); do
    sudo $CTR t list 2>/dev/null | grep -q "$TASK_ID" || break
    sleep 0.1
done
list_after="$(sudo $CTR t list 2>/dev/null || true)"
if echo "$list_after" | grep -q "$TASK_ID"; then
    die "delete 后 task 仍存在"
fi
ok "task 已从列表消失"

# shim 应在 delete 后退出（退出门控：注册表空才 exit.signal）。给它一点时间。
shim_after=0
for _ in $(seq 1 50); do
    shim_after="$(sudo pgrep -f "$SHIM_BIN_NAME.*$NS" | wc -l)"
    [ "$shim_after" -eq 0 ] && break
    sleep 0.1
done
[ "$shim_after" -eq 0 ] || die "shim 进程未退出（退出门控未触发，残留 $shim_after 个）"
ok "shim 进程已退出（退出门控生效）"

echo ""
echo "${C_G}==> PASS: e2e lifecycle (create/start/list/kill/delete)${C_0}"
echo "    containerd↔shim ttrpc 协议链路通畅；MockVm 状态机正确驱动。"
