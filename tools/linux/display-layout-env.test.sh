#!/usr/bin/env bash
# tools/linux/display-layout-env.test.sh
#
# Exercises tools/linux/display-layout-env.sh against fake sysfs/config
# trees. No root, no real hardware, no network. `bash tools/linux/display-
# layout-env.test.sh` (from anywhere; paths below are resolved from this
# file's own location). Exit 0 = pass.
#
# Scenarios: a happy-path layout (the profiles file handed to the
# platform, the render-on GPU resolved); cards renumbered (the render-on
# GPU found by its PCI address on another card number); a render-on GPU
# that is not present; a malformed file (bad render-on, an unknown
# keyword, WM lines after the profiles); a pre-set env var winning over
# the file for each variable; the legacy display-gpu fallback, both when
# it should apply (no display-layout file at all) and when it must not
# (display-layout exists, even without a render-on line); and an
# unreadable-but-present display-layout read as empty rather than
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
# `<unset>` when the helper left it alone: the layout file's override
# (never set here: the backend finds the file itself), the two the
# platform reads and the two from-saved markers (the GPU's and the virtual display's).
# Stderr (the `display-layout:` log lines) is captured to $run_stderr for
# the few cases that check it, and the subshell's own exit status is
# captured to $run_status (expected 0 always: the helper must never fail
# the caller).
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
            printf "%s\n" "${MAKEPAD_DISPLAY_LAYOUT:-<unset>}"
            printf "%s\n" "${MAKEPAD_VULKAN_COMPOSITOR_PCI:-<unset>}"
            printf "%s\n" "${MAKEPAD_WM_GPU_FROM_SAVED:-<unset>}"
            printf "%s\n" "${MAKEPAD_VIRTUAL_DISPLAY:-<unset>}"
            printf "%s\n" "${MAKEPAD_WM_VIRTUAL_FROM_SAVED:-<unset>}"
        ' 2>"$stderr_file")
    run_status=$?
    run_stderr=$(cat "$stderr_file")
    rm -f "$stderr_file"
    IFS=$'\n' read -r -d '' field_layout field_pci field_fromsaved field_virtual field_virtual_fromsaved <<<"$run_stdout"
}

# A layout as the WM writes it: its own lines, then one profile.
layout_text='makepad-display-layout 2
render-on 0000:01:00.0 1234:5678
profile 00000000000000a1,00000000000000b2
screen main
output 0000:01:00.0 DP-1 00000000000000a1 mode=2560x1440@144
screen
output 0000:01:00.0 DP-2 00000000000000b2
pace auto
'

# ---------------------------------------------------------------------
# 1. Happy path: the profiles file is handed to the platform as it is,
#    and the render-on GPU resolves.
# ---------------------------------------------------------------------
s=$(new_scratch)
add_card "$s" 0 0000:00:02.0 8086 a780
add_card "$s" 1 0000:01:00.0 1234 5678
write_layout "$s" "$layout_text"
run_case "$s"
assert_eq "happy-path" "status" "0" "$run_status"
assert_eq "happy-path" "layout (not exported)" "<unset>" "$field_layout"
assert_eq "happy-path" "pci" "0000:01:00.0" "$field_pci"
assert_eq "happy-path" "from_saved" "1" "$field_fromsaved"
rm -rf "$s"

# ---------------------------------------------------------------------
# 2. Cards renumbered: the render-on GPU is found by its PCI address,
#    whichever cardN it is this boot.
# ---------------------------------------------------------------------
s=$(new_scratch)
add_card "$s" 7 0000:01:00.0 1234 5678
add_card "$s" 3 0000:00:02.0 8086 a780
write_layout "$s" "$layout_text"
run_case "$s"
assert_eq "renumbered" "status" "0" "$run_status"
assert_eq "renumbered" "pci" "0000:01:00.0" "$field_pci"
rm -rf "$s"

# ---------------------------------------------------------------------
# 3. The render-on GPU is not present this boot: the automatic GPU, a
#    note on stderr, and the profiles still go to the platform.
# ---------------------------------------------------------------------
s=$(new_scratch)
add_card "$s" 0 0000:00:02.0 8086 a780
write_layout "$s" "$layout_text"
run_case "$s"
assert_eq "missing-gpu" "status" "0" "$run_status"
assert_eq "missing-gpu" "pci" "<unset>" "$field_pci"
assert_eq "missing-gpu" "layout (not exported)" "<unset>" "$field_layout"
case "$run_stderr" in
    *"0000:01:00.0 1234:5678 is not present"*) pass ;;
    *) fail "missing-gpu: expected a stderr note about the missing GPU, got: $run_stderr" ;;
esac
rm -rf "$s"

# ---------------------------------------------------------------------
# 4. A malformed file: a bad render-on (odd token count), an unknown
#    keyword, comments and blank lines, and a WM line after the first
#    profile (the profiles' part, never the WM's). None of this may
#    abort the helper.
# ---------------------------------------------------------------------
s=$(new_scratch)
add_card "$s" 1 0000:01:00.0 1234 5678
write_layout "$s" '# a comment
makepad-display-layout 2

render-on only-one-token
some-unknown-keyword with args here
profile 00000000000000a1
screen main
output 0000:01:00.0 DP-1 00000000000000a1
render-on 0000:01:00.0 1234:5678
virtual 1920x1080@60
'
run_case "$s"
assert_eq "malformed" "status" "0" "$run_status"
assert_eq "malformed" "pci (bad render-on leaves it unset)" "<unset>" "$field_pci"
assert_eq "malformed" "virtual (after the profiles: not the WM's)" "<unset>" "$field_virtual"
rm -rf "$s"

# ---------------------------------------------------------------------
# 5. A pre-set env var wins over the file, for each variable.
# ---------------------------------------------------------------------
s=$(new_scratch)
add_card "$s" 1 0000:01:00.0 1234 5678
write_layout "$s" "$layout_text"
# A set MAKEPAD_DISPLAY_LAYOUT names the file read instead (tests).
printf '%s' "${layout_text/0000:01:00.0 1234:5678/0000:02:00.0 1234:5678}" >"$s/other-layout"
add_card "$s" 2 0000:02:00.0 1234 5678
run_case "$s" MAKEPAD_DISPLAY_LAYOUT="$s/other-layout"
assert_eq "preset-layout" "layout" "$s/other-layout" "$field_layout"
assert_eq "preset-layout" "pci from that file" "0000:02:00.0" "$field_pci"

run_case "$s" MAKEPAD_VULKAN_COMPOSITOR_PCI=keep-me
assert_eq "preset-pci" "pci" "keep-me" "$field_pci"
assert_eq "preset-pci" "from_saved stays unset (not our export)" "<unset>" "$field_fromsaved"
assert_eq "preset-pci" "layout (not exported)" "<unset>" "$field_layout"

run_case "$s" MAKEPAD_VULKAN_COMPOSITOR_UUID=deadbeefdeadbeefdeadbeefdeadbeef
assert_eq "preset-uuid" "pci left unset when UUID is externally pinned" "<unset>" "$field_pci"
rm -rf "$s"

# ---------------------------------------------------------------------
# 6. Legacy display-gpu fallback: only when display-layout does not
#    exist at all.
# ---------------------------------------------------------------------
s=$(new_scratch)
add_card "$s" 1 0000:01:00.0 1234 5678
write_gpu "$s" '0000:01:00.0 1234:5678'
run_case "$s"
assert_eq "legacy-fallback" "status" "0" "$run_status"
assert_eq "legacy-fallback" "pci (from display-gpu)" "0000:01:00.0" "$field_pci"
assert_eq "legacy-fallback" "from_saved" "1" "$field_fromsaved"
rm -rf "$s"

# display-layout exists (even with nothing useful in it) -> the legacy
# file must be ignored even though it names a present, valid GPU.
s=$(new_scratch)
add_card "$s" 1 0000:01:00.0 1234 5678
write_gpu "$s" '0000:01:00.0 1234:5678'
write_layout "$s" 'makepad-display-layout 2
'
run_case "$s"
assert_eq "layout-supersedes-legacy" "status" "0" "$run_status"
assert_eq "layout-supersedes-legacy" "pci (Auto, not the stale legacy file)" "<unset>" "$field_pci"
rm -rf "$s"

# An unreadable-but-present display-layout reads as empty: no legacy
# fallback either (the file's presence, not its readability, decides).
s=$(new_scratch)
add_card "$s" 1 0000:01:00.0 1234 5678
write_gpu "$s" '0000:01:00.0 1234:5678'
write_layout "$s" 'makepad-display-layout 2
render-on 0000:01:00.0 1234:5678
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
# 7. No display-layout and no display-gpu: must not fail; the profiles
#    file is still named (the WM writes it on the first change, and the
#    platform reads it whenever the monitors change), nothing else set.
# ---------------------------------------------------------------------
s=$(new_scratch)
add_card "$s" 0 0000:00:02.0 8086 a780
run_case "$s"
assert_eq "nothing-saved" "status" "0" "$run_status"
assert_eq "nothing-saved" "layout (not exported)" "<unset>" "$field_layout"
assert_eq "nothing-saved" "pci" "<unset>" "$field_pci"
assert_eq "nothing-saved" "virtual" "<unset>" "$field_virtual"
rm -rf "$s"

# ---------------------------------------------------------------------
# 8. No sysfs tree at all under MAKEPAD_SYSFS_DRM (e.g. a stale
#    override): still must not fail.
# ---------------------------------------------------------------------
s=$(new_scratch)
rm -rf "$s/sysfs"
write_layout "$s" "$layout_text"
run_case "$s"
assert_eq "no-sysfs" "status" "0" "$run_status"
assert_eq "no-sysfs" "pci" "<unset>" "$field_pci"
rm -rf "$s"

# ---------------------------------------------------------------------
# 9. The virtual line: exported with its panels word and the marker; the
#    first valid line wins; a malformed one is skipped; a pre-set value
#    wins and clears the marker; absent = unset.
# ---------------------------------------------------------------------
s=$(new_scratch)
write_layout "$s" 'makepad-display-layout 2
virtual 7680x2160@120 panels=off scale=200 follow=on
virtual 1920x1080@60
'
run_case "$s"
assert_eq "virtual" "status" "0" "$run_status"
assert_eq "virtual" "env" "7680x2160@120 off" "$field_virtual"
assert_eq "virtual" "marker" "1" "$field_virtual_fromsaved"
run_case "$s" MAKEPAD_VIRTUAL_DISPLAY="3840x2160@60 mirror" MAKEPAD_WM_VIRTUAL_FROM_SAVED=1
assert_eq "virtual-pinned" "env" "3840x2160@60 mirror" "$field_virtual"
assert_eq "virtual-pinned" "marker cleared" "<unset>" "$field_virtual_fromsaved"
rm -rf "$s"

s=$(new_scratch)
write_layout "$s" 'makepad-display-layout 2
virtual banana
virtual 5120x1440@59.94
'
run_case "$s"
assert_eq "virtual-default-panels" "env" "5120x1440@59.94 mirror" "$field_virtual"
rm -rf "$s"

s=$(new_scratch)
write_layout "$s" 'makepad-display-layout 2
'
run_case "$s"
assert_eq "virtual-absent" "env" "<unset>" "$field_virtual"
assert_eq "virtual-absent" "marker" "<unset>" "$field_virtual_fromsaved"
rm -rf "$s"

echo "display-layout-env.test.sh: $pass_count passed, $fail_count failed"
if [ "$fail_count" -ne 0 ]; then
    exit 1
fi
exit 0
