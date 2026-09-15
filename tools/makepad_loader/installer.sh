#!/bin/sh
# Download this complete file to inspect it, then run: sh makepad-builder.sh
# The final call is outside the function so a truncated pipe cannot run setup.
makepad_main() {
    set -eu
    set +x
    umask 077
    if [ "$(uname -s)" != '@PLATFORM@' ]; then printf '%s\n' 'This download is for @PLATFORM@.' >&2; return 1; fi
    if [ "$(id -u)" = 0 ]; then printf '%s\n' 'Run Builder as your normal user, not root.' >&2; return 1; fi
    exec 3<>/dev/tty || { printf '%s\n' 'An interactive terminal is required.' >&2; return 1; }
    builder_default=${MAKEPAD_LOADER_ROOT:-"$HOME/Makepad"}
    printf '\n  Makepad Builder\n\n  Compile Scope and Makepad apps from source, with your own coding agents.\n\n' >&3
    printf '  Installation folder [%s]: ' "$builder_default" >&3
    IFS= read -r builder_chosen <&3
    builder_root=${builder_chosen:-"$builder_default"}
    case "$builder_root" in '~') builder_root=$HOME ;; '~/'*) builder_root=$HOME/${builder_root#'~/'} ;; esac
    mkdir -p "$builder_root"
    builder_root=$(cd "$builder_root" && pwd -P)
    case "$builder_root" in *'
'*) printf '%s\n' 'Choose a folder without newline characters.' >&3; return 1 ;; esac
    # Do not replace the personal settings of an existing installation.
    if [ ! -e "$builder_root/makepad-builder.json" ]; then
        printf '%s\n' @BOOTSTRAP_SHELL@ > "$builder_root/makepad-builder.json"
    fi
    cat > "$builder_root/check-tools.sh" <<'MAKEPAD_PREFLIGHT'
@PREFLIGHT_SHELL@
MAKEPAD_PREFLIGHT
    cat > "$builder_root/bootstrap-builder.sh" <<'MAKEPAD_BOOTSTRAP'
@BUILDER_BOOTSTRAP@
MAKEPAD_BOOTSTRAP
    cat > "$builder_root/run-builder.sh" <<'MAKEPAD_RUN'
#!/bin/sh
set -eu
builder_root=$(cd -P -- "$(dirname -- "$0")" && pwd)
export MAKEPAD_LOADER_ROOT="$builder_root"
if [ -x "$builder_root/makepad-builder" ]; then
    exec "$builder_root/makepad-builder" tui
fi
exec sh "$builder_root/bootstrap-builder.sh"
MAKEPAD_RUN
    chmod 700 "$builder_root/run-builder.sh" "$builder_root/bootstrap-builder.sh" "$builder_root/check-tools.sh"
    export MAKEPAD_LOADER_ROOT="$builder_root"
    sh "$builder_root/bootstrap-builder.sh" <&3 >&3 2>&3
}
makepad_main
