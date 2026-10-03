# tools/linux/display-layout-env.sh
#
# Sourced by a window manager session script (e.g.
# tools/arch_usb/wm-session.sh) before `target/release/makepad-app-wm` is
# exec'd, to turn the saved
#   ${XDG_CONFIG_HOME:-$HOME/.config}/makepad/wm/display-layout
# file into the env vars the platform reads before Event::Startup
# (platform/src/os/linux/vulkan_linux.rs:new_direct, called before the WM's
# own `Event::Startup`, so the WM cannot set these for its own process):
#
#   MAKEPAD_DISPLAY_ORDER            this-boot output names, left to right
#   MAKEPAD_DRM_MODES                per-screen modes
#   MAKEPAD_VULKAN_COMPOSITOR_PCI    the render-on GPU's PCI address
#
# File format and the screen-key resolution rule are in
# apps/wm/src/shell/display_layout.rs (the pure model). A screen's
# key is (card PCI address, connector name without its `cardN-` prefix);
# resolving it to this boot's name means finding the card whose PCI
# address (the basename of `readlink -f /sys/class/drm/cardN/device`)
# matches, then using `cardN-<connector>` as the name. A screen key that
# does not resolve this boot is dropped, never matched by connector alone.
#
# Safe to source with `set -e`/`set -u` active in the caller: this file
# never calls `exit`, copes with a missing or unreadable `display-layout`
# (an existing-but-unreadable file is treated as empty, exactly as
# `read_display_layout` in system_linux.rs does: the file's *presence*
# decides whether the legacy fallback below applies, not its readability),
# and leaves no function or variable behind once it returns -- everything
# lives in `local`s inside the functions below, and every function this
# file defines is `unset -f` at the end.
#
# `MAKEPAD_SYSFS_DRM` (default `/sys/class/drm`) is the sysfs root used to
# build the `pci -> cardN` map, overridable so this can be tested against
# a fake tree (see tools/linux/display-layout-env.test.sh).
#
# Each of the three env vars above is exported only when it is currently
# unset or empty -- an externally set, non-empty value always wins over
# the file, matching the rule the WM's own runtime restore applies
# (apps/wm/src/shell/display_layout.rs:env_restore_plan).
#
# `render-on` is read from `display-layout` when that file exists (even
# if it has no `render-on` line, meaning "Auto": no fallback then). When
# `display-layout` does not exist at all, this falls back to the legacy
# `display-gpu` file for one release, exactly as the GPU block in
# tools/arch_usb/wm-session.sh (which this helper replaces) did. When the
# helper itself sets `MAKEPAD_VULKAN_COMPOSITOR_PCI` from either source,
# it also sets `MAKEPAD_WM_GPU_FROM_SAVED=1` so the worker's `gpu_env()`
# (system_linux.rs) does not mistake the saved choice for a true external
# override; that flag is always unset first so a stale one from the
# caller's environment cannot persist by accident.
#
# `MAKEPAD_DISPLAY_ORDER` and `MAKEPAD_DRM_MODES` get the same treatment,
# but with one marker each -- `MAKEPAD_WM_ORDER_FROM_SAVED=1` /
# `MAKEPAD_WM_MODES_FROM_SAVED=1` -- rather than a single combined flag,
# because the two variables are independent exports that can disagree
# about which one this helper set from the file and which one was
# already pinned from outside (e.g. a caller that only pins the order).
# A single marker could not tell the two cases apart; the worker reads
# both into `SystemSnapshot::order_env`/`modes_env` (`system_linux.rs`),
# and its runtime restore of the saved layout
# (`display_layout::env_restore_plan`, `linux_controls.rs`) skips the
# Order op entirely while `order_env` is pinned, and a screen's Mode op
# while that screen's name appears in a pinned `modes_env` -- the same
# "an externally set env var always wins" rule this helper already
# applies at session start, also honoured by the WM's own safety-net
# restore. Both markers are always unset first, same reason as the GPU
# one.
#
# One line per export/skip decision is logged to stderr, prefixed
# `display-layout:`.

# `pci-address` as sysfs writes and `validate_gpu_choice` requires it:
# `dddd:bb:ss.f`, 4-8 hex digits for the domain, lowercase.
_mdl_is_pci_address() {
    [[ "$1" =~ ^[0-9a-f]{4,8}:[0-9a-f]{2}:[0-9a-f]{2}\.[0-9a-f]$ ]]
}

# `vvvv:dddd`, lowercase hex, as `validate_gpu_choice` requires.
_mdl_is_pci_ids() {
    [[ "$1" =~ ^[0-9a-f]{4}:[0-9a-f]{4}$ ]]
}

# Mirrors `mode_string_is_valid` (vulkan_linux.rs): `WxH`, `WxH@Hz` or
# `WxH-Hz`, digits only.
_mdl_mode_is_valid() {
    [[ "$1" =~ ^[0-9]+x[0-9]+([@-][0-9]+)?$ ]]
}

# Prints one `<pci> <vendor:device> <cardN>` line per usable card under
# $1 (the sysfs DRM root): a `cardN` entry (not a `cardN-<connector>`
# status directory) whose `device` symlink resolves. Glob-dependent, so
# the caller must not call this while `set -f` (noglob) is in effect.
_mdl_build_card_table() {
    local sysfs_root="$1"
    local card_dir card pci ven dev
    for card_dir in "$sysfs_root"/card[0-9]*; do
        [ -d "$card_dir" ] || continue
        card=$(basename "$card_dir")
        case "$card" in *-*) continue ;; esac
        [ -e "$card_dir/device" ] || continue
        pci=$(readlink -f "$card_dir/device" 2>/dev/null) || continue
        pci=$(basename "$pci" 2>/dev/null) || continue
        [ -n "$pci" ] || continue
        ven=$(cat "$card_dir/device/vendor" 2>/dev/null); ven=${ven#0x}
        dev=$(cat "$card_dir/device/device" 2>/dev/null); dev=${dev#0x}
        printf '%s %s:%s %s\n' "$pci" "$ven" "$dev" "$card"
    done
}

# Prints the `cardN` whose row in $1 (the card table) has pci $2; empty
# and a non-zero return when there is none.
_mdl_card_for_pci() {
    local table="$1" want_pci="$2" row_pci row_ids row_card
    while IFS=' ' read -r row_pci row_ids row_card; do
        [ -n "$row_pci" ] || continue
        if [ "$row_pci" = "$want_pci" ]; then
            printf '%s\n' "$row_card"
            return 0
        fi
    done <<<"$table"
    return 1
}

# True when $1 (the card table) has a row matching both pci $2 and
# vendor:device $3.
_mdl_table_has_match() {
    local table="$1" want_pci="$2" want_ids="$3" row_pci row_ids row_card
    while IFS=' ' read -r row_pci row_ids row_card; do
        [ -n "$row_pci" ] || continue
        if [ "$row_pci" = "$want_pci" ] && [ "$row_ids" = "$want_ids" ]; then
            return 0
        fi
    done <<<"$table"
    return 1
}

# Exports $1=$2 (logged as "$3") unless $1 is already set and non-empty
# (logged instead). Returns non-zero when it left the existing value
# alone, so callers can tell which branch ran.
_mdl_set_env() {
    local name="$1" value="$2" desc="$3" current
    current=${!name:-}
    if [ -n "$current" ]; then
        echo "display-layout: $name already set; leaving it alone" >&2
        return 1
    fi
    export "$name=$value"
    echo "display-layout: $name=$value ($desc)" >&2
    return 0
}

# The legacy `display-gpu` fallback this replaces
# (tools/arch_usb/wm-session.sh's former inline block): exactly one line
# "<pci-address> <vendor>:<device>", with or without a final newline,
# nothing else in the file. Prints "<pci> <ids>" on a match; nothing and
# a non-zero return otherwise.
_mdl_legacy_gpu_choice() {
    local gpu_file="$1" line="" terminated=0 bytes want_pci want_ids
    [ -r "$gpu_file" ] || return 1
    if IFS= read -r line <"$gpu_file"; then
        terminated=1
    fi
    bytes=$(LC_ALL=C wc -c <"$gpu_file" 2>/dev/null | tr -d '[:space:]')
    [ "$bytes" = "$((${#line} + terminated))" ] || return 1
    want_pci=${line% *}
    want_ids=${line#* }
    [ "$want_pci $want_ids" = "$line" ] || return 1
    _mdl_is_pci_address "$want_pci" || return 1
    _mdl_is_pci_ids "$want_ids" || return 1
    printf '%s %s\n' "$want_pci" "$want_ids"
}

# The whole helper, run as the condition of the `if` below so that
# whatever fails inside it (a missing file, a bad sysfs read, ...) never
# trips the caller's `set -e`.
_mdl_main() {
    local cfg_root layout_file gpu_file sysfs_root
    local layout_exists=0 layout_readable=0
    local render_on_pci="" render_on_ids=""
    local order_list="" modes_list=""
    local card_table=""
    local had_noglob=0
    local line keyword pci connector token mode value name card
    local seen_keys=$'\x01'

    cfg_root=${XDG_CONFIG_HOME:-}
    case "$cfg_root" in
        /*) ;;
        *) cfg_root=${HOME:-}/.config ;;
    esac
    layout_file="$cfg_root/makepad/wm/display-layout"
    gpu_file="$cfg_root/makepad/wm/display-gpu"
    sysfs_root=${MAKEPAD_SYSFS_DRM:-/sys/class/drm}

    # Glob-enabled: build the pci -> cardN(+ids) map once, before any file
    # content is word-split.
    card_table=$(_mdl_build_card_table "$sysfs_root")

    [ -e "$layout_file" ] && layout_exists=1
    [ -r "$layout_file" ] && layout_readable=1

    if [ "$layout_exists" = 1 ] && [ "$layout_readable" = 0 ]; then
        echo "display-layout: $layout_file exists but is not readable; treating it as empty" >&2
    fi

    if [ "$layout_readable" = 1 ]; then
        case "$-" in *f*) had_noglob=1 ;; esac
        set -f
        while IFS= read -r line || [ -n "$line" ]; do
            line=${line%$'\r'}
            set -- $line
            [ "$#" -gt 0 ] || continue
            keyword=$1
            case "$keyword" in
                '#'*) continue ;;
                makepad-display-layout) continue ;;
                render-on)
                    if [ -n "$render_on_pci" ]; then
                        continue
                    fi
                    if [ "$#" -ne 3 ]; then
                        echo "display-layout: malformed render-on line; skipped" >&2
                        continue
                    fi
                    if _mdl_is_pci_address "$2" && _mdl_is_pci_ids "$3"; then
                        render_on_pci=$2
                        render_on_ids=$3
                    else
                        echo "display-layout: malformed render-on line; skipped" >&2
                    fi
                    ;;
                screen)
                    if [ "$#" -lt 3 ]; then
                        echo "display-layout: malformed screen line; skipped" >&2
                        continue
                    fi
                    pci=$2
                    connector=$3
                    case "$seen_keys" in
                        *"${pci}|${connector}"$'\x01'*)
                            continue
                            ;;
                    esac
                    seen_keys="${seen_keys}${pci}|${connector}"$'\x01'
                    shift 3
                    mode=""
                    for token in "$@"; do
                        case "$token" in
                            mode=*)
                                value=${token#mode=}
                                if _mdl_mode_is_valid "$value"; then
                                    mode="$value"
                                fi
                                ;;
                        esac
                    done
                    card=$(_mdl_card_for_pci "$card_table" "$pci") || card=""
                    if [ -z "$card" ]; then
                        echo "display-layout: screen $pci $connector not present this boot; dropped" >&2
                        continue
                    fi
                    name="${card}-${connector}"
                    if [ -z "$order_list" ]; then
                        order_list="$name"
                    else
                        order_list="$order_list,$name"
                    fi
                    if [ -n "$mode" ]; then
                        if [ -z "$modes_list" ]; then
                            modes_list="$name=$mode"
                        else
                            modes_list="$modes_list,$name=$mode"
                        fi
                    fi
                    ;;
                *) continue ;;
            esac
        done <"$layout_file"
        [ "$had_noglob" = 1 ] || set +f
    fi

    unset MAKEPAD_WM_ORDER_FROM_SAVED
    if [ -n "$order_list" ]; then
        if _mdl_set_env MAKEPAD_DISPLAY_ORDER "$order_list" "from display-layout"; then
            export MAKEPAD_WM_ORDER_FROM_SAVED=1
        fi
    else
        echo "display-layout: no resolvable screen order; MAKEPAD_DISPLAY_ORDER left to the platform" >&2
    fi

    unset MAKEPAD_WM_MODES_FROM_SAVED
    if [ -n "$modes_list" ]; then
        if _mdl_set_env MAKEPAD_DRM_MODES "$modes_list" "from display-layout"; then
            export MAKEPAD_WM_MODES_FROM_SAVED=1
        fi
    else
        echo "display-layout: no saved modes; MAKEPAD_DRM_MODES left to the platform" >&2
    fi

    # render-on: display-layout (even with no render-on line, meaning
    # Auto) always decides once it exists; the legacy display-gpu file is
    # only consulted when display-layout does not exist at all -- the
    # same rule system_linux.rs's read_display_layout applies.
    if [ "$layout_exists" = 0 ]; then
        local legacy
        legacy=$(_mdl_legacy_gpu_choice "$gpu_file") || legacy=""
        if [ -n "$legacy" ]; then
            render_on_pci=${legacy% *}
            render_on_ids=${legacy#* }
        fi
    fi

    unset MAKEPAD_WM_GPU_FROM_SAVED
    if [ -n "${MAKEPAD_VULKAN_COMPOSITOR_PCI:-}" ] || [ -n "${MAKEPAD_VULKAN_COMPOSITOR_UUID:-}" ]; then
        echo "display-layout: MAKEPAD_VULKAN_COMPOSITOR_PCI/_UUID already set; leaving the render-on GPU alone" >&2
    elif [ -n "$render_on_pci" ]; then
        if _mdl_table_has_match "$card_table" "$render_on_pci" "$render_on_ids"; then
            export MAKEPAD_WM_GPU_FROM_SAVED=1
            export MAKEPAD_VULKAN_COMPOSITOR_PCI="$render_on_pci"
            echo "display-layout: MAKEPAD_VULKAN_COMPOSITOR_PCI=$render_on_pci (saved render-on GPU)" >&2
        else
            echo "display-layout: saved render-on GPU $render_on_pci $render_on_ids is not present on this machine; using the automatic GPU" >&2
        fi
    else
        echo "display-layout: no saved render-on GPU; using the automatic GPU" >&2
    fi

    return 0
}

if ! _mdl_main; then
    echo "display-layout: internal error; leaving the environment as it was" >&2
fi

unset -f _mdl_main _mdl_set_env _mdl_legacy_gpu_choice _mdl_table_has_match \
    _mdl_card_for_pci _mdl_build_card_table _mdl_mode_is_valid _mdl_is_pci_ids \
    _mdl_is_pci_address
