#!/bin/bash
# The controller only reads Git and dispatches leases. No builds run here.
exec python3 -I "$(dirname "$0")/verify_tip.py" "$@"
