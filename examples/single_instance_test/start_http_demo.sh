#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")" && pwd)"
WEB_DIR="$ROOT/web"
PORT="${1:-8000}"

if command -v python3 >/dev/null 2>&1; then
  PYTHON=python3
elif command -v python >/dev/null 2>&1; then
  PYTHON=python
else
  echo "python or python3 is required" >&2
  exit 1
fi

HOST_IP="$($PYTHON - <<'PY'
import socket
ip = '127.0.0.1'
try:
    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    s.connect(('8.8.8.8', 80))
    ip = s.getsockname()[0]
    s.close()
except Exception:
    pass
print(ip)
PY
)"

cat <<EOF
Serving $WEB_DIR
Local:   http://127.0.0.1:${PORT}/
LAN:     http://${HOST_IP}:${PORT}/

Open the LAN URL on your test devices.
Press Ctrl-C to stop.
EOF

cd "$WEB_DIR"
exec "$PYTHON" -m http.server "$PORT" --bind 0.0.0.0
