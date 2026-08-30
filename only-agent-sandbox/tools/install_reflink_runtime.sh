#!/usr/bin/env bash
# 以可回滚方式安装本次 OAS reflink/CoW runtime 和配置。
#
# 该脚本必须由部署节点 root 执行；默认不重启 OAS，避免把服务重启隐含在
# 构建步骤中。它会先备份 config 和旧 binary，再写入三项配置：
# reflink_enabled=true、reflink_required=true、rw_template_path=...
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CONFIG_PATH="${OAS_CONFIG_PATH:-/etc/oas/config.toml}"
RUNTIME_BIN="${OAS_RUNTIME_BIN:-$SCRIPT_DIR/../assets/oas-runtime-reflink}"
INSTALL_BIN="${OAS_INSTALL_BIN:-/usr/local/bin/oas-runtime}"
TEMPLATE_PATH="${OAS_RW_TEMPLATE:-/var/lib/oas/artifacts/rw-template.ext4}"
TEMPLATE_SIZE_MIB="${OAS_RW_SIZE_MIB:-1024}"

[[ "$(id -u)" == 0 ]] || { echo "run as root" >&2; exit 1; }
[[ -f "$CONFIG_PATH" ]] || { echo "config not found: $CONFIG_PATH" >&2; exit 1; }
[[ -x "$RUNTIME_BIN" ]] || { echo "runtime binary not executable: $RUNTIME_BIN" >&2; exit 1; }

MOUNT_INFO="$(findmnt -T "$(dirname "$TEMPLATE_PATH")" -o FSTYPE,OPTIONS -n 2>/dev/null || true)"
grep -Eq '^(xfs|btrfs)[[:space:]]' <<<"$MOUNT_INFO" || {
    echo "rw template must be on XFS or btrfs: $MOUNT_INFO" >&2
    exit 1
}
grep -q 'rw[, ]' <<<"$MOUNT_INFO" || {
    echo "rw template filesystem is not writable: $MOUNT_INFO" >&2
    exit 1
}

STAMP="$(date +%Y%m%d_%H%M%S)"
cp -a "$CONFIG_PATH" "$CONFIG_PATH.bak.$STAMP"
if [[ -e "$INSTALL_BIN" ]]; then cp -a "$INSTALL_BIN" "$INSTALL_BIN.bak.$STAMP"; fi

# 模板只创建一次；已存在的模板不会被覆盖。
if [[ ! -e "$TEMPLATE_PATH" ]]; then
    "$SCRIPT_DIR/prepare_rw_template.sh" "$TEMPLATE_PATH" "$TEMPLATE_SIZE_MIB"
fi

TMP_CONFIG="$(mktemp "${CONFIG_PATH}.tmp.XXXXXX")"
trap 'rm -f "$TMP_CONFIG"' EXIT
# 删除旧的三项顶层键，再插入新值；插入位置在 [net] 之前，避免落入 TOML table。
awk -v template="$TEMPLATE_PATH" '
BEGIN { inserted=0 }
/^reflink_enabled[[:space:]]*=/ { next }
/^reflink_required[[:space:]]*=/ { next }
/^rw_template_path[[:space:]]*=/ { next }
!inserted && /^\[net\][[:space:]]*$/ {
    print "# OAS reflink/CoW runtime policy (installed by install_reflink_runtime.sh)"
    print "reflink_enabled = true"
    print "reflink_required = true"
    print "rw_template_path = \"" template "\""
    inserted=1
}
{ print }
END { if (!inserted) exit 2 }
' "$CONFIG_PATH" > "$TMP_CONFIG"
install -m 0644 "$TMP_CONFIG" "$CONFIG_PATH"
install -m 0755 "$RUNTIME_BIN" "$INSTALL_BIN"

echo "installed runtime: $INSTALL_BIN"
echo "updated config: $CONFIG_PATH"
echo "rw template: $TEMPLATE_PATH"
echo "backup suffix: .$STAMP"
echo "restart OAS service explicitly after reviewing the backup and config."
