#!/bin/bash
set -eu
exec python3 "$(dirname "$0")/run.py" "$@"
