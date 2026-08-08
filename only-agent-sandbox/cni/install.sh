#!/usr/bin/env bash
# Install oas-vm-net + oas-cni-invoke into CNI bin dir (default /opt/cni/bin).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")" && pwd)"
DEST="${1:-/opt/cni/bin}"
mkdir -p "$DEST"
install -m 0755 "$ROOT/oas-vm-net/oas-vm-net" "$DEST/oas-vm-net"
install -m 0755 "$ROOT/oas-cni-invoke/oas-cni-invoke" "$DEST/oas-cni-invoke"
echo "installed:"
ls -l "$DEST/oas-vm-net" "$DEST/oas-cni-invoke"
