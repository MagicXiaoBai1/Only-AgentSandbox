#!/usr/bin/env bash
# Validate an immutable OAS snapshot bundle before publishing or restoring it.
set -euo pipefail

BUNDLE_DIR="${1:?usage: $0 <bundle-dir>}"
[[ -d "$BUNDLE_DIR" ]] || { echo "bundle dir not found: $BUNDLE_DIR" >&2; exit 1; }

for file in vmlinux rootfs.ext4 vmstate mem manifest.json SHA256SUMS; do
    [[ -s "$BUNDLE_DIR/$file" ]] || { echo "missing or empty bundle file: $file" >&2; exit 1; }
done

grep -qx '"schema_version": 1,' "$BUNDLE_DIR/manifest.json" || {
    echo "unsupported or missing manifest schema" >&2; exit 1;
}
(
    cd "$BUNDLE_DIR"
    sha256sum -c SHA256SUMS
)
echo "bundle verified: $BUNDLE_DIR"
