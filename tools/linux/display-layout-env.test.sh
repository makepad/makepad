#!/usr/bin/env bash
# tools/linux/display-layout-env.test.sh
#
# Exercises tools/linux/display-layout-env.sh against fake sysfs/config
# trees. No root, no real hardware, no network. `bash tools/linux/display-
# layout-env.test.sh` (from anywhere; paths below are resolved from this
# file's own location). Exit 0 = pass.
#
# Scenarios: a happy-path layout; cards renumbered (same pci, different
# cardN than a hypothetical earlier boot); a screen whose card is missing
# this boot; a malformed file (bad render-on, a too-short screen line, an
# unknown keyword, a duplicate screen key, an invalid mode=); a pre-set
# env var winning over the file for each of the three variables, and the
# per-variable MAKEPAD_WM_ORDER_FROM_SAVED/MAKEPAD_WM_MODES_FROM_SAVED
# markers that go with MAKEPAD_DISPLAY_ORDER/MAKEPAD_DRM_MODES (set only
# when *this* var was set from the file, cleared when it was left alone,
# and never left stale from a previous process even when the caller's
# environment already carried one); the legacy display-gpu fallback,
# both when it should apply (no display-layout file at all) and when it
# must not (display-layout exists, even without a render-on line); and
# an unreadable-but-present display-layout read as empty rather than
# falling back.

set -u

script_dir=$(cd "$(dirname "$0")" && pwd)
helper="$script_dir/display-layout-env.sh"

pass_count=0
fail_count=0

fail() {
    fail_count=$((fail_count + 1))
    echo "FAIL: $1" >&2
}

pass() {
    pass_count=$((pass_count + 1))
}

assert_eq() {
    # $1 = case label, $2 = field, $3 = expected, $4 = actual
    if [ "$3" = "$4" ]; then
        pass
    else
        fail "$1: $2 expected '$3' got '$4'"
    fi
}

# Builds a fresh scratch dir with empty sysfs/ and cfg/ subtrees; echoes
# its path.
new_scratch() {
    local dir
    dir=$(mktemp -d "${TMPDIR:-/tmp}/mdl-test.XXXXXX")
    mkdir -p "$dir/sysfs" "$dir/cfg/makepad/wm"
    printf '%s\n' "$dir"
}

# add_card SCRATCH CARDNUM PCI VENDOR_HEX DEVICE_HEX
# Creates sysfs/cardN/device -> a fake PCI device directory carrying
# vendor/device files in sysfs's own "0x"-prefixed form, so the helper's
# own `${ven#0x}` stripping is exercised exactly as it is on real sysfs.
add_card() {
    local scratch="$1" num="$2" pci="$3" ven="$4" dev="$5"
    local pci_dir="$scratch/pci-devices/$pci"
    mkdir -p "$pci_dir" "$scratch/sysfs/card$num"
    printf '0x%s' "$ven" >"$pci_dir/vendor"
    printf '0x%s' "$dev" >"$pci_dir/device"
    ln -s "$pci_dir" "$scratch/sysfs/card$num/device"
}

# write_layout SCRATCH TEXT
write_layout() {
    printf '%s' "$2" >"$1/cfg/makepad/wm/display-layout"
}

# write_gpu SCRATCH TEXT (no trailing newline, matching the legacy
# file's own one-ASCII-line shape)
write_gpu() {
    printf '%s' "$2" >"$1/cfg/makepad/wm/display-gpu"
}

# run_case SCRATCH [EXTRA_ENV_ASSIGNMENT ...]
# Sources the helper in a brand-new, otherwise-empty bash process (so no
# variable from this test script, or from a previous case, can leak in)
# and prints the production code's variables, one per line, each
# `<unset>` when the helper left it alone: the three the platform reads,
# and the three from-saved markers (one for the GPU, one each for order
# and modes). Stderr (the `display-layout:` log lines) is captured to
# $run_stderr for the few cases that check it, and the subshell's own
# exit status is captured to $run_status (expected 0 always: the helper
# must never fail the caller).
run_stdout=""
run_stderr=""
run_status=0
run_case() {
    local scratch="$1"
    shift
    local stderr_file
    stderr_file=$(mktemp "${TMPDIR:-/tmp}/mdl-test-stderr.XXXXXX")
    run_stdout=$(env -i \
        PATH="$PATH" \
        HOME="$scratch/home-unused" \
        HELPER="$helper" \
        MAKEPAD_SYSFS_DRM="$scratch/sysfs" \
        XDG_CONFIG_HOME="$scratch/cfg" \
        "$@" \
        bash -c '
            set -eu
            . "$HELPER"
            printf "%s\n" "${MAKEPAD_DISPLAY_ORDER:-<unset>}"
            printf "%s\n" "${MAKEPAD_DRM_MODES:-<unset>}"
            printf "%s\n" "${MAKEPAD_VULKAN_COMPOSITOR_PCI:-<unset>}"
            printf "%s\n" "${MAKEPAD_WM_GPU_FROM_SAVED:-<unset>}"
            printf "%s\n" "${MAKEPAD_WM_ORDER_FROM_SAVED:-<unset>}"
            printf "%s\n" "${MAKEPAD_WM_MODES_FROM_SAVED:-<unset>}"
        ' 2>"$stderr_file")
    run_status=$?
    run_stderr=$(cat "$stderr_file")
    rm -f "$stderr_file"
    IFS=$'\n' read -r -d '' field_order field_modes field_pci field_fromsaved \
        field_order_fromsaved field_modes_fromsaved <<<"$run_stdout"
}

# ---------------------------------------------------------------------
# 1. Happy path: two screens, one with a mode, a render-on GPU.
# ---------------------------------------------------------------------
s=$(new_scratch)
add_card "$s" 0 0000:00:02.0 8086 a780
add_card "$s" 1 0000:01:00.0 10de 2b85
write_layout "$s" 'makepad-display-layout 1
render-on 0000:01:00.0 10de:2b85
screen 0000:00:02.0 HDMI-A-2 mode=3840x2160@30
screen 0000:01:00.0 HDMI-A-1 main
'
run_case "$s"
assert_eq "happy-path" "status" "0" "$run_status"
assert_eq "happy-path" "order" "card0-HDMI-A-2,card1-HDMI-A-1" "$field_order"
assert_eq "happy-path" "modes" "card0-HDMI-A-2=3840x2160@30" "$field_modes"
assert_eq "happy-path" "pci" "0000:01:00.0" "$field_pci"
assert_eq "happy-path" "from_saved" "1" "$field_fromsaved"
assert_eq "happy-path" "order_from_saved" "1" "$field_order_fromsaved"
assert_eq "happy-path" "modes_from_saved" "1" "$field_modes_fromsaved"
rm -rf "$s"

# ---------------------------------------------------------------------
# 2. Cards renumbered: the same pci addresses as case 1, but assigned to
#    different card numbers this boot. Resolution must follow pci, not
#    any assumption about which cardN a screen had last time.
# ---------------------------------------------------------------------
s=$(new_scratch)
add_card "$s" 7 0000:01:00.0 10de 2b85
add_card "$s" 3 0000:00:02.0 8086 a780
write_layout "$s" 'makepad-display-layout 1
render-on 0000:01:00.0 10de:2b85
screen 0000:00:02.0 HDMI-A-2 mode=3840x2160@30
screen 0000:01:00.0 HDMI-A-1 main
'
run_case "$s"
assert_eq "renumbered" "status" "0" "$run_status"
assert_eq "renumbered" "order" "card3-HDMI-A-2,card7-HDMI-A-1" "$field_order"
assert_eq "renumbered" "modes" "card3-HDMI-A-2=3840x2160@30" "$field_modes"
assert_eq "renumbered" "pci" "0000:01:00.0" "$field_pci"
rm -rf "$s"

# ---------------------------------------------------------------------
# 3. A missing card: the saved HDMI-A-1 screen's GPU is not present this
#    boot. It must be dropped (never matched by connector alone), the
#    other screen still resolves, and the helper does not fail.
# ---------------------------------------------------------------------
s=$(new_scratch)
add_card "$s" 0 0000:00:02.0 8086 a780
write_layout "$s" 'makepad-display-layout 1
screen 0000:00:02.0 HDMI-A-2 mode=3840x2160@30
screen 0000:01:00.0 HDMI-A-1 main
'
run_case "$s"
assert_eq "missing-card" "status" "0" "$run_status"
assert_eq "missing-card" "order" "card0-HDMI-A-2" "$field_order"
assert_eq "missing-card" "modes" "card0-HDMI-A-2=3840x2160@30" "$field_modes"
case "$run_stderr" in
    *"0000:01:00.0 HDMI-A-1 not present this boot"*) pass ;;
    *) fail "missing-card: expected a stderr note about the dropped screen, got: $run_stderr" ;;
esac
rm -rf "$s"

# ---------------------------------------------------------------------
# 4. A malformed file: a bad render-on (odd token count), a too-short
#    screen line, an unknown keyword, a duplicate screen key (the
#    second copy's mode must lose to the first's), an invalid mode=,
#    and ordinary comments/blank lines all mixed in with valid lines.
#    None of this may abort the helper.
# ---------------------------------------------------------------------
s=$(new_scratch)
add_card "$s" 0 0000:00:02.0 8086 a780
add_card "$s" 1 0000:01:00.0 10de 2b85
write_layout "$s" '# a comment
makepad-display-layout 1

render-on only-one-token
some-unknown-keyword with args here
screen
screen 0000:00:02.0 HDMI-A-2 mode=3840x2160@30
screen 0000:00:02.0 HDMI-A-2 mode=1920x1080
screen 0000:01:00.0 HDMI-A-1 mode=not-a-mode
'
run_case "$s"
assert_eq "malformed" "status" "0" "$run_status"
assert_eq "malformed" "order" "card0-HDMI-A-2,card1-HDMI-A-1" "$field_order"
assert_eq "malformed" "modes (first screen wins over its duplicate)" "card0-HDMI-A-2=3840x2160@30" "$field_modes"
assert_eq "malformed" "pci (bad render-on leaves it unset)" "<unset>" "$field_pci"
rm -rf "$s"

# ---------------------------------------------------------------------
# 5. A pre-set env var wins over the file, for each of the three
#    variables independently.
# ---------------------------------------------------------------------
s=$(new_scratch)
add_card "$s" 0 0000:00:02.0 8086 a780
add_card "$s" 1 0000:01:00.0 10de 2b85
write_layout "$s" 'makepad-display-layout 1
render-on 0000:01:00.0 10de:2b85
screen 0000:00:02.0 HDMI-A-2 mode=3840x2160@30
screen 0000:01:00.0 HDMI-A-1 main
'
run_case "$s" MAKEPAD_DISPLAY_ORDER=keep-me
assert_eq "preset-order" "order" "keep-me" "$field_order"
assert_eq "preset-order" "order_from_saved stays unset (not our export)" "<unset>" "$field_order_fromsaved"
assert_eq "preset-order" "modes still applied" "card0-HDMI-A-2=3840x2160@30" "$field_modes"
assert_eq "preset-order" "modes_from_saved still set (independent of the order pin)" "1" "$field_modes_fromsaved"
assert_eq "preset-order" "pci still applied" "0000:01:00.0" "$field_pci"

run_case "$s" MAKEPAD_DRM_MODES=keep-me
assert_eq "preset-modes" "modes" "keep-me" "$field_modes"
assert_eq "preset-modes" "modes_from_saved stays unset (not our export)" "<unset>" "$field_modes_fromsaved"
assert_eq "preset-modes" "order still applied" "card0-HDMI-A-2,card1-HDMI-A-1" "$field_order"
assert_eq "preset-modes" "order_from_saved still set (independent of the modes pin)" "1" "$field_order_fromsaved"

# A caller whose environment already carries a stale from-saved marker
# (e.g. inherited from an earlier, unrelated process) must not have it
# survive once the var itself is externally pinned: both markers are
# unset first, before either var is even looked at.
run_case "$s" MAKEPAD_DISPLAY_ORDER=keep-me MAKEPAD_WM_ORDER_FROM_SAVED=1
assert_eq "preset-order-stale-marker" "order_from_saved is cleared, not left stale" "<unset>" "$field_order_fromsaved"
run_case "$s" MAKEPAD_DRM_MODES=keep-me MAKEPAD_WM_MODES_FROM_SAVED=1
assert_eq "preset-modes-stale-marker" "modes_from_saved is cleared, not left stale" "<unset>" "$field_modes_fromsaved"

run_case "$s" MAKEPAD_VULKAN_COMPOSITOR_PCI=keep-me
assert_eq "preset-pci" "pci" "keep-me" "$field_pci"
assert_eq "preset-pci" "from_saved stays unset (not our export)" "<unset>" "$field_fromsaved"
assert_eq "preset-pci" "order still applied" "card0-HDMI-A-2,card1-HDMI-A-1" "$field_order"

run_case "$s" MAKEPAD_VULKAN_COMPOSITOR_UUID=deadbeefdeadbeefdeadbeefdeadbeef
assert_eq "preset-uuid" "pci left unset when UUID is externally pinned" "<unset>" "$field_pci"
rm -rf "$s"

# ---------------------------------------------------------------------
# 6. Legacy display-gpu fallback: only when display-layout does not
#    exist at all.
# ---------------------------------------------------------------------
s=$(new_scratch)
add_card "$s" 1 0000:01:00.0 10de 2b85
write_gpu "$s" '0000:01:00.0 10de:2b85'
run_case "$s"
assert_eq "legacy-fallback" "status" "0" "$run_status"
assert_eq "legacy-fallback" "pci (from display-gpu)" "0000:01:00.0" "$field_pci"
assert_eq "legacy-fallback" "from_saved" "1" "$field_fromsaved"
rm -rf "$s"

# display-layout exists (even with nothing useful in it) -> the legacy
# file must be ignored even though it names a present, valid GPU.
s=$(new_scratch)
add_card "$s" 1 0000:01:00.0 10de 2b85
write_gpu "$s" '0000:01:00.0 10de:2b85'
write_layout "$s" 'makepad-display-layout 1
'
run_case "$s"
assert_eq "layout-supersedes-legacy" "status" "0" "$run_status"
assert_eq "layout-supersedes-legacy" "pci (Auto, not the stale legacy file)" "<unset>" "$field_pci"
rm -rf "$s"

# An unreadable-but-present display-layout reads as empty: no legacy
# fallback either (the file's presence, not its readability, decides).
s=$(new_scratch)
add_card "$s" 1 0000:01:00.0 10de 2b85
write_gpu "$s" '0000:01:00.0 10de:2b85'
write_layout "$s" 'makepad-display-layout 1
render-on 0000:01:00.0 10de:2b85
'
chmod 000 "$s/cfg/makepad/wm/display-layout"
if [ -r "$s/cfg/makepad/wm/display-layout" ]; then
    echo "SKIP: unreadable-layout case needs a non-root user to enforce file permissions" >&2
else
    run_case "$s"
    assert_eq "unreadable-layout" "status" "0" "$run_status"
    assert_eq "unreadable-layout" "pci (no fallback to the legacy file)" "<unset>" "$field_pci"
fi
chmod 600 "$s/cfg/makepad/wm/display-layout"
rm -rf "$s"

# ---------------------------------------------------------------------
# 7. No display-layout and no display-gpu: must not fail, nothing set.
# ---------------------------------------------------------------------
s=$(new_scratch)
add_card "$s" 0 0000:00:02.0 8086 a780
run_case "$s"
assert_eq "nothing-saved" "status" "0" "$run_status"
assert_eq "nothing-saved" "order" "<unset>" "$field_order"
assert_eq "nothing-saved" "modes" "<unset>" "$field_modes"
assert_eq "nothing-saved" "pci" "<unset>" "$field_pci"
rm -rf "$s"

# ---------------------------------------------------------------------
# 8. No sysfs tree at all under MAKEPAD_SYSFS_DRM (e.g. a stale
#    override): still must not fail.
# ---------------------------------------------------------------------
s=$(new_scratch)
rm -rf "$s/sysfs"
write_layout "$s" 'makepad-display-layout 1
screen 0000:00:02.0 HDMI-A-2
'
run_case "$s"
assert_eq "no-sysfs" "status" "0" "$run_status"
assert_eq "no-sysfs" "order" "<unset>" "$field_order"
rm -rf "$s"

echo "display-layout-env.test.sh: $pass_count passed, $fail_count failed"
if [ "$fail_count" -ne 0 ]; then
    exit 1
fi
exit 0
