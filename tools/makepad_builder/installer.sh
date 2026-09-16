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
    # Linux: GPU driver notice first, before the folder is created or anything
    # is downloaded. The bootstrap and menu skip their copy of the question
    # for this invocation through MAKEPAD_GPU_ACK. macOS is unchanged.
    if [ "$(uname -s)" = Linux ]; then
        printf '  Makepad Scope heavily uses the GPU to render its UI. If you have old or\n  broken video drivers, this can result in a system crash.\n' >&3
        while :; do
            printf '  Do you want to continue? [Y/n] ' >&3
            IFS= read -r builder_answer <&3 || { printf '\n  Setup cancelled. Nothing was downloaded or installed.\n' >&3; return 1; }
            case "$builder_answer" in
                ''|y|Y|yes|Yes) break ;;
                n|N|no|No|"$(printf '\033')"*) printf '  Setup cancelled. Nothing was downloaded or installed.\n' >&3; return 1 ;;
                *) printf '  Please answer y or n.\n' >&3 ;;
            esac
        done
        export MAKEPAD_GPU_ACK=1
        printf '\n' >&3
    fi
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
    cat > "$builder_root/rust-tools.sh" <<'MAKEPAD_RUST_TOOLS'
@RUST_TOOLS_SHELL@
MAKEPAD_RUST_TOOLS
    cat > "$builder_root/bootstrap-builder.sh" <<'MAKEPAD_BOOTSTRAP'
@BUILDER_BOOTSTRAP@
MAKEPAD_BOOTSTRAP
    cat > "$builder_root/run-builder.sh" <<'MAKEPAD_RUN'
#!/bin/sh
# The bootstrap re-checks the recorded compiler choice on every start and
# opens the menu directly when nothing needs attention.
set -eu
builder_root=$(cd -P -- "$(dirname -- "$0")" && pwd)
export MAKEPAD_LOADER_ROOT="$builder_root"
exec sh "$builder_root/bootstrap-builder.sh"
MAKEPAD_RUN
    chmod 700 "$builder_root/run-builder.sh" "$builder_root/bootstrap-builder.sh" "$builder_root/check-tools.sh" "$builder_root/rust-tools.sh"
    export MAKEPAD_LOADER_ROOT="$builder_root"
    sh "$builder_root/bootstrap-builder.sh" <&3 >&3 2>&3
}
makepad_main
