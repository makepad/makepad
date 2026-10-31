#!/usr/bin/env bash
# Opt one native Linux or Steam/Proton launch into this checkout's XR runtime.
# Does not install software or replace the user's default OpenXR runtime.
set -euo pipefail
if [[ $# -eq 0 ]]; then
    printf 'Usage: %s <game command> [arguments...]\nSteam launch options: %s %%command%%\n' "$0" "$0" >&2
    exit 2
fi
xr_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)
xr_library="$xr_root/target/release/libmakepad_xr_runtime.so"
if [[ ! -f "$xr_library" ]]; then
    printf 'Build the runtime first: cargo build --release -p makepad-xr-runtime\n' >&2
    exit 1
fi
xr_manifest="$xr_root/target/release/makepad-xr-runtime.json"
python3 - "$xr_manifest" "$xr_library" <<'PY'
import json, os, sys, tempfile
path, library = sys.argv[1:]
with tempfile.NamedTemporaryFile(mode='w', dir=os.path.dirname(path), delete=False) as f:
    temporary = f.name
    json.dump({'file_format_version': '1.0.0', 'runtime': {'library_path': library}}, f)
    f.write('\n')
os.replace(temporary, path)
PY
export XR_RUNTIME_JSON="$xr_manifest"
export PRESSURE_VESSEL_IMPORT_OPENXR_1_RUNTIMES=1
export MAKEPAD_XR_SOCKET="${MAKEPAD_XR_SOCKET:-${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/makepad-xr.sock}"
# Steam gives games a private runtime directory. Importing the OpenXR manifest
# does not also expose the bridge's Unix socket to the game container.
export PRESSURE_VESSEL_FILESYSTEMS_RW="${PRESSURE_VESSEL_FILESYSTEMS_RW:+${PRESSURE_VESSEL_FILESYSTEMS_RW}:}$MAKEPAD_XR_SOCKET"
xr_command=("$@")
for ((xr_i=0; xr_i+2<${#xr_command[@]}; xr_i++)); do
    if [[ "${xr_command[xr_i]##*/}" == proton ]] && [[ "${xr_command[xr_i+1]}" == run || "${xr_command[xr_i+1]}" == waitforexitandrun ]]; then
        xr_helper="$xr_root/target/x86_64-pc-windows-msvc/release/makepad-xr-proton.exe"
        if [[ ! -f "$xr_helper" ]]; then
            printf 'Build the Proton OpenXR helper first: tools/linux/build-xr-proton.sh\n' >&2
            exit 1
        fi
        xr_game_index=$((xr_i+2))
        xr_game="${xr_command[xr_game_index]}"
        # Proton converts the helper executable's Unix path. The helper passes
        # the original game's path to CreateProcessW, so give it a Wine path.
        if [[ "$xr_game" == /* ]]; then xr_game="Z:${xr_game//\//\\}"; fi
        xr_command=("${xr_command[@]:0:xr_game_index}" "$xr_helper" "$xr_game" "${xr_command[@]:xr_game_index+1}")
        break
    fi
done
exec "${xr_command[@]}"
