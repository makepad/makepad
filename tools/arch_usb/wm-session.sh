#!/usr/bin/env bash
set -Eeuo pipefail
if test -f /etc/makepad/wm.env; then
    set -a
    source /etc/makepad/wm.env
    set +a
fi
export MAKEPAD=linux_direct+vulkan
export MAKEPAD_WM_ROOT=${MAKEPAD_WM_ROOT:-/home/arch/makepad}
export CARGO_NET_OFFLINE=true
export MAKEPAD_CEF_OFFLINE=1
export XDG_RUNTIME_DIR=${XDG_RUNTIME_DIR:-/run/user/$(id -u)}
cd "$MAKEPAD_WM_ROOT"
if test ! -x target/release/wm || test "${MAKEPAD_WM_BUILD:-0}" = 1; then
    cargo build --offline --release -p makepad-wm
fi

# The GPU the compositor starts on. The WM's Display panel saves the
# chosen GPU's identity in the user's config as exactly one ASCII line
#   "<pci-address> <vendor>:<device>"      e.g. "0000:01:00.0 10de:2b85"
# (sysfs's lowercase hex, one space), with or without a final newline.
# Card numbers are not stable across boots, so the identity is resolved
# here by PCI address AND vendor:device, nothing looser, and handed to
# the WM as MAKEPAD_VULKAN_COMPOSITOR_PCI=<pci> plus
# MAKEPAD_WM_GPU_FROM_SAVED=1 so the worker does not treat that as an
# external override. A connected display is not required: compositing
# may run on a GPU that only drives compute. MAKEPAD_DRM_DEVICE is the
# display output and is left alone. An explicit
# MAKEPAD_VULKAN_COMPOSITOR_UUID or MAKEPAD_VULKAN_COMPOSITOR_PCI in the
# environment (or /etc/makepad/wm.env) wins, is left alone, and keeps
# the saved-choice marker unset. A file that is missing or malformed,
# or an identity not present on this machine (a stick moved to another
# PC), warns and falls back to the renderer's automatic choice. Nothing
# here touches CUDA. The WM's worker validates the same identity shape
# (system_linux.rs validate_gpu_choice).
unset MAKEPAD_WM_GPU_FROM_SAVED
wm_gpu_pci=""
if test -z "${MAKEPAD_VULKAN_COMPOSITOR_UUID:-}" && test -z "${MAKEPAD_VULKAN_COMPOSITOR_PCI:-}"; then
    wm_cfg_root="${XDG_CONFIG_HOME:-}"
    case "$wm_cfg_root" in
        /*) ;;
        *) wm_cfg_root="$HOME/.config" ;;
    esac
    wm_gpu_file="$wm_cfg_root/makepad/wm/display-gpu"
    if test -r "$wm_gpu_file"; then
        wm_gpu_line=""
        wm_gpu_terminated=0
        if IFS= read -r wm_gpu_line < "$wm_gpu_file"; then
            wm_gpu_terminated=1
        fi
        wm_gpu_bytes=$(LC_ALL=C wc -c < "$wm_gpu_file" | tr -d '[:space:]')
        wm_gpu_re='^([0-9a-f]{4,8}:[0-9a-f]{2}:[0-9a-f]{2}\.[0-9a-f]) ([0-9a-f]{4}:[0-9a-f]{4})$'
        if test "$wm_gpu_bytes" -ne "$(( ${#wm_gpu_line} + wm_gpu_terminated ))" || ! [[ "$wm_gpu_line" =~ $wm_gpu_re ]]; then
            echo "wm-session: $wm_gpu_file is not one line '<pci-address> <vendor>:<device>'; using the automatic GPU" >&2
        else
            want_pci=${BASH_REMATCH[1]}
            want_ids=${BASH_REMATCH[2]}
            chosen=""
            for card_dir in /sys/class/drm/card[0-9]*; do
                card=$(basename "$card_dir")
                case "$card" in *-*) continue ;; esac
                test -e "$card_dir/device" || continue
                pci=$(basename "$(readlink -f "$card_dir/device")")
                ven=$(cat "$card_dir/device/vendor" 2>/dev/null || true)
                dev=$(cat "$card_dir/device/device" 2>/dev/null || true)
                ids="${ven#0x}:${dev#0x}"
                if test "$pci" = "$want_pci" && test "$ids" = "$want_ids"; then
                    chosen=$card
                    break
                fi
            done
            if test -z "$chosen"; then
                echo "wm-session: saved GPU '$want_pci $want_ids' is not present on this machine; using the automatic GPU" >&2
            else
                wm_gpu_pci=$want_pci
            fi
        fi
    fi
fi
if test -n "$wm_gpu_pci"; then
    echo "wm-session: compositor PCI $wm_gpu_pci (saved GPU choice)" >&2
    exec env MAKEPAD_WM_GPU_FROM_SAVED=1 MAKEPAD_VULKAN_COMPOSITOR_PCI="$wm_gpu_pci" target/release/wm "-scale=${MAKEPAD_WM_SCALE:-1.3}" "$@"
fi
exec target/release/wm "-scale=${MAKEPAD_WM_SCALE:-1.3}" "$@"
