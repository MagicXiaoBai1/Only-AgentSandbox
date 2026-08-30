#!/usr/bin/env bash
# 创建 OAS 运行时使用的预格式化 ext4 rw 模板。
#
# 每个 sandbox 不再执行 truncate+mkfs.ext4；RealStorageManager 会在 XFS 上
# 通过 FICLONE 从本模板派生实例文件，guest 后续写入由 XFS CoW 分裂块。
# 模板必须与 rw_base_dir 位于同一个支持 reflink 的文件系统上。
#
# 用法：
#   sudo ./tools/prepare_rw_template.sh [output.ext4] [size_mib]
set -euo pipefail

TEMPLATE="${1:-/var/lib/oas/artifacts/rw-template.ext4}"
SIZE_MIB="${2:-1024}"

[[ "$SIZE_MIB" =~ ^[1-9][0-9]*$ ]] || {
    echo "size_mib must be a positive integer: $SIZE_MIB" >&2
    exit 2
}
[[ ! -e "$TEMPLATE" ]] || {
    echo "refusing to overwrite existing template: $TEMPLATE" >&2
    exit 1
}

mkdir -p "$(dirname "$TEMPLATE")"
# 只在模板上执行一次格式化；实例复制阶段不会再次运行 mkfs。
truncate -s "${SIZE_MIB}M" "$TEMPLATE"
mkfs.ext4 -F "$TEMPLATE"
chmod 0444 "$TEMPLATE"
echo "prepared rw CoW template: $TEMPLATE (${SIZE_MIB} MiB)"
