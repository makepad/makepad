#!/bin/sh
# WM usability gate (node 165, hidden): ~25 plain user tasks against the real
# WM and real apps, checked by the WM's motion recorder. A few minutes.
#   apps/wm/tools/usability/run.sh [checkout with a release build] [suite.py args]
# Needs: cargo build --release -p makepad-app-wm (plus the app binaries) in
# the checkout; weston; python3.
here=$(cd "$(dirname "$0")" && pwd)
root=${1:-$(cd "$here/../../../.." && pwd)}
[ $# -gt 0 ] && shift
exec python3 "$here/suite.py" "$root" "$@"
