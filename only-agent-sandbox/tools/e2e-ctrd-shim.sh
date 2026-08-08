#!/usr/bin/env bash
# oas-ctrd-shim 端到端测试：起一个隔离的 containerd 1.6.33 实例，用 ctr --rootfs 驱动
# `containerd-shim-oas-v2` 走完一条 task 的 create/start/list/kill/delete 全链路。
#
# 为什么隔离：本机系统 containerd 正在跑真实业务（moby + k8s.io），绝不能碰
# /run/containerd。本脚本起独立 containerd，socket/root/state 全落 /tmp 下，互不干扰。
#
# 两种模式（由 OAS_VM 环境变量切换）：
#   OAS_VM=mock (默认)：下层 MockVm——不起真实进程，pid 是占位值 424242。
#       只验证 containerd↔shim 的 ttrpc 协议链路与状态机，不验证真实 workload。
#       断言 task RUNNING + pid=424242 + kill/delete 后 shim 退出。
#   OAS_VM=real：下层 RealVm——经 oas_driver::vm_core 拉起真实 firecracker 做 snapshot 恢复。
#       预建 netns+tapH0、生成 OAS_CONFIG（type0→base-1-agent、关 guest-agent 等待）。
#       断言 task RUNNING + pid≠424242（真实 fc host pid）+ 真实 firecracker 进程出现，
#       且 kill/delete 后被清掉。in-VM exec 仍不在范围（见 ADR 0011）。
#
# containerd 经 Legacy start 协议拉起 shim，shim::parse 不透传 --config（见 ADR 0010），
# 故 real 模式把 OAS_CONFIG 放进 containerd 进程环境，shim 作为 containerd 子进程继承之。
#
# 用法：
#   sudo ./tools/e2e-ctrd-shim.sh                # mock 模式（协议级，无需 firecracker）
#   sudo OAS_VM=real ./tools/e2e-ctrd-shim.sh    # real 模式（需 firecracker/jailer + bundle）
#   TEST_KEEP=1 sudo OAS_VM=real ./tools/e2e-ctrd-shim.sh   # 保留现场便于排查
#
# 启动耗时测量（real 模式）：末尾打印 containerd 就绪 / shim 启动(ctr run→RUNNING) /
# teardown 三段 wall-clock，并解析 shim 日志（OAS_SHIM_LOG）的 log_step 行给出
# materialize/jailer_spawn/wait_fc_socket/snapshot_load/… 各阶段毫秒拆分。
# 前提：shim 已装 tracing 订阅器（run_server::init_tracing）且 action_start 把常驻子进程
# stderr 重定向到 OAS_SHIM_LOG——否则 log_step 事件被丢弃，拆分段为空。
set -euo pipefail

KEEP="${TEST_KEEP:-0}"
MODE="${OAS_VM:-mock}"   # mock | real
[ "$MODE" = "mock" ] || [ "$MODE" = "real" ] || { echo "FAIL: OAS_VM 只能是 mock 或 real"; exit 1; }

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
SHIM_LOG="$WORK/logs/shim-$TASK_ID.log"   # real 模式下 vm_core 的 tracing 输出经 containerd 转发

SHIM_BIN="$REPO_DIR/target/debug/$SHIM_BIN_NAME"
CTR="ctr -a $SOCK -n $NS"

# real 模式专属路径
ARTIFACTS="${ARTIFACTS:-/var/lib/oas/artifacts}"
BUNDLE="${BUNDLE:-base-1-agent}"                 # 含 guest-agent 的已烘焙 bundle
# jailer --chroot-base-dir。**必须在非 nodev 文件系统上**：jailer 在 chroot 内 mknod
# /dev/kvm，而 nodev 挂载（如 /tmp 的 tmpfs）会让设备节点不可开 → KVM EACCES。
# /var/lib/oas 在根 xfs（无 nodev），且与 artifacts 同 fs → reflink 瞬时 materialize。
CHROOT_BASE="${CHROOT_BASE:-/var/lib/oas/e2e-chroot}"
NETNS="oas-$TASK_ID"                              # cfg.netns_name(sid) 派生
NETNS_PATH="/var/run/netns/$NETNS"                # cfg.netns_path(sid) 派生
TAP="${TAP:-tapH0}"
GATEWAY="${GATEWAY:-172.16.0.1}"
GUEST_IP="${GUEST_IP:-172.16.0.2}"
PREFIX="${PREFIX:-30}"
OAS_CFG="$WORK/oas-config.toml"

# 颜色（非 tty 自动降级）
if [ -t 1 ]; then
    C_G=$'\033[32m'; C_R=$'\033[31m'; C_Y=$'\033[33m'; C_B=$'\033[34m'; C_0=$'\033[0m'
else
    C_G=''; C_R=''; C_Y=''; C_B=''; C_0=''
fi

ok()    { echo "    ${C_G}OK${C_0}: $*"; }
step()  { echo "${C_B}==>${C_0} $*"; }
die()   { echo "${C_R}FAIL${C_0}: $*"; exit 1; }

# shim 进程详情（区分 start 父进程 vs run 常驻子进程）。
# 注意：pgrep -f 会把自己的命令行（含 "containerd-shim-oas-v2"）也匹配上 → 必须过滤掉 pgrep/pkill 自身。
show_shim_procs() {
    echo "    -- shim 进程 (pgrep -af) --"
    local lines
    lines=$(sudo pgrep -af "$SHIM_BIN_NAME" 2>/dev/null | grep -v -E '[p]grep|pkill')
    if [ -n "$lines" ]; then echo "$lines" | sed 's/^/      /'; else echo "      (无)"; fi
    echo "    -- containerd 日志中 shim 相关行 --"
    grep -iE "shim|oas\.v2|runtime.v2" "$CTRD_LOG" 2>/dev/null | tail -15 | sed 's/^/      /' || true
}

# shim 进程数（排除 pgrep/pkill 自身匹配）。pgrep 无匹配时返回 1，加 || true 兼容 pipefail。
count_shim() { sudo pgrep -af "$SHIM_BIN_NAME" 2>/dev/null | grep -v -E '[p]grep|pkill' | wc -l || true; }

# real 模式：firecracker 进程数。
count_fc() { pgrep -a firecracker 2>/dev/null | wc -l || true; }

# ---- 计时（高精度 wall-clock，秒浮点；bash 无浮点算术 → awk）-----------------
# 测三段：containerd 就绪 / shim 启动(ctr run→RUNNING) / teardown(kill+delete→退出)。
# real 模式另解析 shim 的 log_step 日志，给出 materialize/jailer/snapshot_load 等阶段拆分。
now() { date +%s.%N; }
elapsed() { awk -v a="$1" -v b="$2" 'BEGIN{d=b-a; if(d<0)d=0; printf "%.3f", d}'; }
T_CD0=""; T_CD1=""; T_RUN0=""; T_RUN1=""; T_TD0=""; T_TD1=""

# ---- 清理 -------------------------------------------------------------------
cleanup() {
    set +e
    if [ "$KEEP" = "1" ]; then
        echo ""
        echo "${C_Y}[keep]${C_0} 现场保留："
        echo "  containerd sock : $SOCK"
        echo "  containerd log  : $CTRD_LOG"
        echo "  work dir        : $WORK"
        [ -f "$OAS_CFG" ] && echo "  oas config      : $OAS_CFG"
        [ "$MODE" = "real" ] && echo "  netns           : $NETNS ($NETNS_PATH)"
        [ -f "$SHIM_LOG" ] && echo "  shim log        : $SHIM_LOG"
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
    # real 模式：兜底杀残留 firecracker + 清 netns
    if [ "$MODE" = "real" ]; then
        pkill -9 firecracker 2>/dev/null || true
        sudo ip netns del "$NETNS" 2>/dev/null || true
        sudo rm -rf "$CHROOT_BASE"
    fi
    sudo rm -rf "$WORK"
}
trap cleanup EXIT INT TERM

# ---- 0. 前置检查 ------------------------------------------------------------
step "前置检查 (mode=$MODE)"
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
# rootfs 留空目录即可：VM 的 rootfs 来自 snapshot bundle（rootfs.ext4），非 container rootfs。

# containerd 1.6.x 对 shim 二进制有属主/权限校验 → root 属主 0755 安装到 opt/bin。
sudo install -m 0755 -o root -g root "$SHIM_BIN" "$WORK/opt/bin/$SHIM_BIN_NAME"
ok "shim 已安装到 $WORK/opt/bin/$SHIM_BIN_NAME (root:root 0755)"

# ---- 1b. real 模式：预建 netns+tap + 生成 OAS_CONFIG ------------------------
if [ "$MODE" = "real" ]; then
    step "real 模式：预建 netns+tap 与 OAS_CONFIG"
    FC_BIN="${FC_BIN:-/home/yunfei/workspace/snap_double_shot/bin/firecracker}"
    JAILER_BIN="${JAILER_BIN:-/home/yunfei/workspace/snap_double_shot/bin/jailer}"
    BUNDLE_DIR="$ARTIFACTS/snapshots/$BUNDLE"
    [ -x "$FC_BIN" ]     || die "firecracker 不存在: $FC_BIN"
    [ -x "$JAILER_BIN" ] || die "jailer 不存在: $JAILER_BIN"
    [ -f "$BUNDLE_DIR/vmstate" ] || die "bundle 未烘焙: $BUNDLE_DIR（缺 vmstate）"
    ok "bundle=$BUNDLE_DIR (vmlinux/rootfs.ext4/vmstate/mem 齐全)"

    # jailer 要求 chroot-base-dir 为 root:root 0755。
    sudo mkdir -p "$CHROOT_BASE"
    sudo chown root:root "$CHROOT_BASE"
    sudo chmod 0755 "$CHROOT_BASE"

    # firecracker（uid 1234）需可写 API logger 路径（cfg.log_dir/fc-<sid>.log）。
    sudo chmod 0777 "$WORK/logs"

    # per-VM 可写 ext4：bundle 快照内置 virtio-block → ./data.ext4，须 materialize 进 jail。
    # 镜像 oas-storage::provision / test_shim.sh：truncate 1G + mkfs.ext4。
    RW_DIR="${RW_BASE:-/var/lib/oas/rw}"
    sudo mkdir -p "$RW_DIR"
    RW_FILE="$RW_DIR/$TASK_ID.ext4"
    sudo rm -f "$RW_FILE"
    sudo truncate -s 1G "$RW_FILE"
    sudo mkfs.ext4 -F "$RW_FILE" >/dev/null
    ok "rw layer: $RW_FILE (1G ext4 → jail /data.ext4)"

    # netns + tapH0 + 网关（镜像 oas-net::setup / test_shim.sh）。
    # 先删可能残留的同名 netns（忽略错误），再干净创建。
    sudo ip netns del "$NETNS" 2>/dev/null || true
    sudo ip netns add "$NETNS"
    sudo ip netns exec "$NETNS" ip tuntap add dev "$TAP" mode tap
    sudo ip netns exec "$NETNS" ip addr add "$GATEWAY/$PREFIX" dev "$TAP"
    sudo ip netns exec "$NETNS" ip link set "$TAP" up
    ok "netns=$NETNS tap=$TAP gw=$GATEWAY/$PREFIX"

    # 生成 OAS_CONFIG：type0→$BUNDLE、关 guest-agent 等待（恢复链路验证不强依赖 agent）。
    # 其余字段与 oas-config::Config::default() 对齐，仅改 bundle / wait_guest_agent / 路径。
    cat > "$OAS_CFG" <<EOF
firecracker_bin = "$FC_BIN"
jailer_bin = "$JAILER_BIN"
chroot_base_dir = "$CHROOT_BASE"
jailer_uid = 1234
jailer_gid = 1234
artifacts_dir = "$ARTIFACTS"
run_base_dir = "/run/oas"
log_dir = "$WORK/logs"
store_path = "$WORK/store.redb"
cri_socket = "$WORK/oas.sock"
rw_base_dir = "/var/lib/oas/rw"
rw_size_mib = 1024

[net]
tap_name = "$TAP"
tap_gateway = "$GATEWAY"
tap_prefix = $PREFIX
guest_ip = "$GUEST_IP"
guest_mac = "06:00:AC:10:00:02"
pod_cidr = "10.244.0.0/24"
pod_gateway = "10.244.0.254"
guest_agent_port = 10000
enable_host_veth = true
pod_iface = "eth0"
enable_guest_egress = true
wait_guest_agent = false
guest_agent_wait_secs = 30

[[types]]
type_id = 0
bundle = "$BUNDLE"
vcpu = 2
mem_mib = 1024
has_rw_layer = true
has_cloud_disk = false
image_whitelist = ["guest-agent"]
EOF
    ok "OAS_CONFIG: $OAS_CFG"
fi

# ---- 2. 写最小 containerd config.toml ---------------------------------------
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
# 把 OAS_VM / OAS_CONFIG 注入 containerd 环境：shim 作为子进程继承之（见 ADR 0010）。
#   mock 模式：OAS_VM=mock → shim 走 MockVm。
#   real 模式：OAS_CONFIG=<path>、不设 OAS_VM → shim 走 RealVm。
if [ "$MODE" = "mock" ]; then
    CTRD_ENV=(sudo env "PATH=$WORK/opt/bin:$PATH" "OAS_VM=mock")
else
    # OAS_SHIM_LOG：常驻 shim 子进程 stderr 重定向到此文件（见 start::shim_stderr），
    # run_server 安装的 tracing 订阅器把 log_step 各阶段耗时写进来，供末尾解析。
    # RUST_LOG=info：EnvFilter 全局 info，覆盖 target="oas-shim" 的 log_step(info 级)。
    CTRD_ENV=(sudo env "PATH=$WORK/opt/bin:$PATH" "OAS_CONFIG=$OAS_CFG" \
        "OAS_SHIM_LOG=$SHIM_LOG" "RUST_LOG=info")
fi
"${CTRD_ENV[@]}" containerd -c "$WORK/config.toml" --address "$SOCK" \
    > "$CTRD_LOG" 2>&1 &
CTRD_PID=$!
echo "$CTRD_PID" > "$CTRD_PID_FILE"
echo "    containerd pid=$CTRD_PID, log=$CTRD_LOG"

# 轮询等 socket 就绪
T_CD0=$(now)
ready=0
for _ in $(seq 1 100); do
    if sudo $CTR version >/dev/null 2>&1; then ready=1; break; fi
    sleep 0.1
done
T_CD1=$(now)
[ "$ready" = "1" ] || { echo "    containerd 启动失败，日志尾部:"; tail -40 "$CTRD_LOG"; die "containerd socket 未就绪"; }
ok "containerd 就绪: $SOCK"

# ---- 4. 创建并启动 task -----------------------------------------------------
step "ctr run --rootfs (runtime=$RUNTIME, mode=$MODE)"
# --rootfs  : 免镜像/免 registry，OCI spec root.path 指向空目录（VM rootfs 来自 bundle）。
# --null-io : shim 不开 FIFO（无真实 host 侧容器进程开 io）。
# -d         : detach，不在 task.Wait 上挂（wait=(b)：仅 stop 后返回）。
# /bin/true  : 仅填充 OCI spec process.args，RealVm/MockVm 都不会真正 exec 它。
# 注意：ctr 的 urfave/cli 不支持 flag 与位置参数交错——所有 flag 必须在位置参数之前。
# real 模式 create 内含 materialize+jailer+snapshot/load，可能数秒；ctr run 会阻塞到 Running。
T_RUN0=$(now)
sudo $CTR run -d --runtime "$RUNTIME" --null-io --rootfs "$ROOTFS" "$TASK_ID" /bin/true \
    || {
        echo "    ctr run 失败，containerd 日志尾部:"
        tail -50 "$CTRD_LOG"
        if [ "$MODE" = "real" ]; then
            echo "    -- firecracker 日志 (API logger) --"
            [ -f "$WORK/logs/fc-$TASK_ID.log" ] && tail -40 "$WORK/logs/fc-$TASK_ID.log" | sed 's/^/      /' || echo "      (无 fc-$TASK_ID.log)"
            echo "    -- jail root 内 fc 日志 --"
            fc_jail="$CHROOT_BASE/firecracker/$TASK_ID/root/fc-$TASK_ID.log"
            [ -f "$fc_jail" ] && tail -40 "$fc_jail" | sed 's/^/      /' || echo "      (无 $fc_jail)"
        fi
        die "ctr run 失败"
    }
ok "task 已创建并启动: $TASK_ID"
T_RUN1=$(now)

# 给 containerd 一点时间拉起 shim 并完成 create/start
for _ in $(seq 1 100); do
    sudo $CTR t list 2>/dev/null | grep -q "$TASK_ID" && break
    sleep 0.1
done

# ---- 5. 断言状态 ------------------------------------------------------------
step "断言 task 状态 (mode=$MODE)"
list_out="$(sudo $CTR t list 2>/dev/null || true)"
echo "$list_out" | sed 's/^/    | /'

echo "$list_out" | grep -q "$TASK_ID" || { show_shim_procs; die "task 未出现在 ctr t list"; }
echo "$list_out" | grep -q "RUNNING"  || { show_shim_procs; die "task 未进入 RUNNING"; }

# 提取 pid（ctr t list 第二列）。
TASK_PID="$(echo "$list_out" | awk -v t="$TASK_ID" '$1==t{print $2}' | head -1)"
[ -n "$TASK_PID" ] || die "无法解析 task pid（ctr t list 输出异常）"

if [ "$MODE" = "mock" ]; then
    [ "$TASK_PID" = "424242" ] || die "mock 模式 task pid != 424242（MockVm 占位 pid 未透传），got=$TASK_PID"
    ok "task=$TASK_ID state=RUNNING pid=424242 (mock)"
else
    [ "$TASK_PID" != "424242" ] || die "real 模式 task pid 仍是占位 424242（RealVm 未生效？）"
    [ "$TASK_PID" -gt 1 ] 2>/dev/null || die "real 模式 task pid 非法: $TASK_PID"
    ok "task=$TASK_ID state=RUNNING pid=$TASK_PID (真实 fc host pid, ≠424242)"
    # 真实 firecracker 进程应存在。
    fc_cnt="$(count_fc)"
    [ "$fc_cnt" -ge 1 ] || { echo "    FAIL: 未找到 firecracker 进程"; die "real 模式：firecracker 未拉起"; }
    ok "firecracker 进程存在: $fc_cnt 个"
    pgrep -a firecracker | sed 's/^/      | /' || true
fi

# shim 常驻进程应存在（仅匹配本 namespace，不误伤系统）
shim_cnt="$(count_shim)"
[ "$shim_cnt" -ge 1 ] || { show_shim_procs; die "未找到 shim 进程（pgrep $SHIM_BIN_NAME.*$NS）"; }
ok "shim 常驻进程存在: $shim_cnt 个"
show_shim_procs

# ---- 6. kill + delete -------------------------------------------------------
step "ctr t kill + delete"
T_TD0=$(now)
sudo $CTR t kill "$TASK_ID" || die "ctr t kill 失败"
ok "kill 完成（stop → cleanup + wake waiters）"

# delete 可能需要 task 先 stop；kill 后应可删，必要时 --force
sudo $CTR t delete "$TASK_ID" 2>/dev/null || sudo $CTR t delete --force "$TASK_ID" \
    || { echo "    containerd 日志尾部:"; tail -40 "$CTRD_LOG"; die "ctr t delete 失败"; }
ok "delete 完成（注册表清空 → exit.signal）"

# ---- 7. 断言清理结果 --------------------------------------------------------
step "断言清理结果 (mode=$MODE)"
for _ in $(seq 1 50); do
    sudo $CTR t list 2>/dev/null | grep -q "$TASK_ID" || break
    sleep 0.1
done
list_after="$(sudo $CTR t list 2>/dev/null || true)"
if echo "$list_after" | grep -q "$TASK_ID"; then
    die "delete 后 task 仍存在"
fi
ok "task 已从列表消失"

# real 模式：firecracker 进程应被 cleanup 终结。
if [ "$MODE" = "real" ]; then
    fc_after=1
    for _ in $(seq 1 50); do
        fc_after="$(count_fc)"
        [ "$fc_after" -eq 0 ] && break
        sleep 0.1
    done
    [ "$fc_after" -eq 0 ] || die "firecracker 进程未退出（cleanup 未杀掉，残留 $fc_after 个）"
    ok "firecracker 进程已退出（cleanup 生效）"
fi

# shim 应在 delete 后退出（退出门控：注册表空才 exit.signal）。
shim_after=1
for _ in $(seq 1 50); do
    shim_after="$(count_shim)"
    [ "$shim_after" -eq 0 ] && break
    sleep 0.1
done
[ "$shim_after" -eq 0 ] || { show_shim_procs; die "shim 进程未退出（退出门控未触发，残留 $shim_after 个）"; }
ok "shim 进程已退出（退出门控生效）"
T_TD1=$(now)

# ---- 8. 启动耗时报告 --------------------------------------------------------
step "启动耗时报告 (mode=$MODE)"
cd_ms=$(elapsed "$T_CD0" "$T_CD1")
run_ms=$(elapsed "$T_RUN0" "$T_RUN1")
td_ms=$(elapsed "$T_TD0" "$T_TD1")
printf "    %-26s %8s s\n" "containerd 就绪" "$cd_ms"
printf "    %-26s %8s s   <-- shim 启动 (ctr run → RUNNING)\n" "shim_startup" "$run_ms"
printf "    %-26s %8s s\n" "teardown (kill+delete)" "$td_ms"

# real 模式：解析 shim 日志的 log_step 阶段拆分（materialize/jailer/snapshot_load …）。
if [ "$MODE" = "real" ]; then
    echo "    -- shim 阶段拆分 (log_step, target=oas-shim, from $SHIM_LOG) --"
    if [ ! -f "$SHIM_LOG" ]; then
        echo "      (无 shim 日志：OAS_SHIM_LOG 未生效或 shim 未装订阅器)"
    else
        # || true 防 set -e+pipefail 在 grep 无匹配(返回1)时直接退出脚本。
        parsed="$( { grep -oE 'step=[^ ]+ elapsed_ms=[0-9]+' "$SHIM_LOG" 2>/dev/null || true; } \
                    | sed 's/step=//; s/ elapsed_ms=/ /')"
        if [ -z "$parsed" ]; then
            echo "      (未解析到 step=/elapsed_ms= 行，shim 日志原文前 20 行：)"
            sed -n '1,20p' "$SHIM_LOG" 2>/dev/null | sed 's/^/        /'
        else
            printf "      %-22s %10s\n" "阶段" "ms"
            while read -r nm ms; do
                [ -n "$nm" ] || continue
                printf "      %-22s %10s\n" "$nm" "$ms"
            done <<< "$parsed"
            # wall-clock 与 fresh_restore_total 的差 ≈ containerd task RPC + start liveness 开销。
            fr_total="$(awk '$1=="fresh_restore_total"{print $2}' <<<"$parsed")"
            if [ -n "$fr_total" ]; then
                overhead_ms=$(awk -v w="$run_ms" -v f="$fr_total" 'BEGIN{printf "%.0f", w*1000-f}')
                printf "      %-22s %10s\n" "RPC+liveness 开销(估)" "$overhead_ms"
            fi
        fi
    fi
fi

echo ""
if [ "$MODE" = "mock" ]; then
    echo "${C_G}==> PASS: e2e lifecycle (mock)${C_0}"
    echo "    containerd↔shim ttrpc 协议链路通畅；MockVm 状态机正确驱动。"
else
    echo "${C_G}==> PASS: e2e lifecycle (real)${C_0}"
    echo "    真实 firecracker snapshot 恢复链路通畅：vm_core materialize+jailer+snapshot/load → Running；"
    echo "    真实 pid 透传；kill+delete 后 firecracker 与 shim 均退出。"
fi
