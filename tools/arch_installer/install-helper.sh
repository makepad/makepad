#!/usr/bin/env bash
# Install into the running system, or a mounted installer root passed as $1.
set -Eeuo pipefail
root=${1:-/}
source_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
test "$(id -u)" = 0
test -d "$root/etc"
install -d -m 0755 "$root/usr/lib/makepad-arch-installer" "$root/etc/makepad"
install -o root -g root -m 0755 "$source_dir/helper.py" "$root/usr/lib/makepad-arch-installer/helper.py"
# The helper still requires that / is on a physical USB disk with this filesystem UUID.
uuid=$(findmnt -n -o UUID -T "$root")
test -n "$uuid"
python3 - "$root/etc/makepad/installer-source.json" "$uuid" <<'PY'
import json, sys
from pathlib import Path
Path(sys.argv[1]).write_text(json.dumps({'version': 1, 'root_uuid': sys.argv[2]}) + '\n')
PY
chmod 0644 "$root/etc/makepad/installer-source.json"
