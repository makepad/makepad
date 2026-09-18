#!/bin/sh
# Download this complete file to inspect it, then run: sh makepad-builder.sh
# The final call is outside the function so a truncated pipe cannot run setup.

# Resolve a possibly not-yet-created folder without creating anything. The
# existing portion is resolved physically so a symlink to $HOME cannot bypass
# the dedicated-folder check; missing tail components are then appended with
# . and .. normalized.
builder_canonical_path() {
    builder_path=$1
    case "$builder_path" in
        /*) ;;
        *) builder_path=$(pwd -P)/$builder_path ;;
    esac
    while [ "$builder_path" != / ] && [ "${builder_path%/}" != "$builder_path" ]; do
        builder_path=${builder_path%/}
    done
    builder_suffix=
    while [ "$builder_path" != / ] && [ ! -e "$builder_path" ] && [ ! -L "$builder_path" ]; do
        builder_component=${builder_path##*/}
        [ -n "$builder_component" ] || return 1
        builder_suffix=/$builder_component$builder_suffix
        builder_parent=${builder_path%/*}
        [ -n "$builder_parent" ] || builder_parent=/
        builder_path=$builder_parent
    done
    builder_path=$(cd -P -- "$builder_path" 2>/dev/null && pwd -P) || return 1
    builder_result=$builder_path
    builder_remaining=${builder_suffix#/}
    while [ -n "$builder_remaining" ]; do
        builder_component=${builder_remaining%%/*}
        if [ "$builder_remaining" = "$builder_component" ]; then
            builder_remaining=
        else
            builder_remaining=${builder_remaining#*/}
        fi
        case "$builder_component" in
            ''|.) ;;
            ..)
                case "$builder_result" in
                    /) ;;
                    */*) builder_result=${builder_result%/*}; [ -n "$builder_result" ] || builder_result=/ ;;
                esac
                ;;
            *)
                case "$builder_result" in
                    /) builder_result=/$builder_component ;;
                    *) builder_result=$builder_result/$builder_component ;;
                esac
                ;;
        esac
    done
    printf '%s\n' "$builder_result"
}

builder_validate_install_root() {
    builder_candidate=$(builder_canonical_path "$1") || return 1
    builder_home=$(builder_canonical_path "$HOME") || return 1
    case "$builder_candidate" in
        /|"$builder_home") return 1 ;;
    esac
    printf '%s\n' "$builder_candidate"
}

makepad_main() {
    set -eu
    set +x
    umask 077
    if [ "$(uname -s)" != '@PLATFORM@' ]; then printf '%s\n' 'This download is for @PLATFORM@.' >&2; return 1; fi
    if [ "$(id -u)" = 0 ]; then printf '%s\n' 'Run Builder as your normal user, not root.' >&2; return 1; fi
    exec 3<>/dev/tty || { printf '%s\n' 'An interactive terminal is required.' >&2; return 1; }
    builder_default=${MAKEPAD_LOADER_ROOT:-"$HOME/Makepad"}
    builder_fallback_default=$HOME/Makepad
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
    while :; do
        printf '  Installation folder [%s]: ' "$builder_default" >&3
        if ! IFS= read -r builder_chosen <&3; then
            printf '\n  Setup cancelled. Nothing was downloaded or installed.\n' >&3
            return 1
        fi
        builder_root=${builder_chosen:-"$builder_default"}
        case "$builder_root" in '~') builder_root=$HOME ;; '~/'*) builder_root=$HOME/${builder_root#'~/'} ;; esac
        case "$builder_root" in *'
'*)
            printf '\n  Choose a folder without newline characters.\n' >&3
            builder_default=$builder_fallback_default
            continue
            ;;
        esac
        if ! builder_root=$(builder_canonical_path "$builder_root") || ! builder_root=$(builder_validate_install_root "$builder_root"); then
            printf '\n  Builder needs a dedicated installation subfolder.\n  It cannot use / or your home folder directly.\n  Suggested folder: %s\n' "$builder_fallback_default" >&3
            builder_default=$builder_fallback_default
            continue
        fi
        if ! mkdir -p "$builder_root"; then
            printf '\n  Could not create %s. Choose another folder.\n' "$builder_root" >&3
            builder_default=$builder_fallback_default
            continue
        fi
        if ! builder_root=$(builder_validate_install_root "$builder_root"); then
            printf '\n  The selected folder resolved to / or your home folder.\n  Choose a dedicated subfolder such as %s.\n' "$builder_fallback_default" >&3
            builder_default=$builder_fallback_default
            continue
        fi
        break
    done
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
