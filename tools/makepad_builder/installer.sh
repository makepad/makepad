#!/bin/sh
# Makepad Builder for @PLATFORM@ (macOS = Darwin).
#
# You can read this whole file before running it:
#   curl -fsSL https://makepad.nl/builder/macos -o makepad-builder.sh   (or /linux)
#   sh makepad-builder.sh
#
# What it does:
#   1. asks which folder to use (default ~/makepad-commercial);
#   2. writes four small scripts into that folder (their full text is below);
#   3. runs bootstrap-builder.sh there, which checks your build tools, sets up
#      Rust, fetches the Makepad source and compiles and starts the Builder.
# It never deletes anything and changes nothing outside that folder, except
# the development packages on Linux, which it shows and asks about first.
# Your email is not in this file; the Builder asks for it and keeps it in the
# folder.
#
# Everything runs from main() on the very last line, so a download that was
# cut off halfway does nothing at all.

main() {
    set -eu
    umask 077

    [ "$(uname -s)" = '@PLATFORM@' ] || { echo 'This download is for @PLATFORM@.' >&2; return 1; }
    [ "$(id -u)" != 0 ] || { echo 'Run the Builder as your normal user, not as root.' >&2; return 1; }
    # Piped through sh, standard input is this script: talk to the terminal.
    exec 3<>/dev/tty || { echo 'The Builder needs an interactive terminal.' >&2; return 1; }

    printf '\n  Makepad Builder\n\n' >&3
    printf '  Makepad apps ship as source, so your own coding agent can change them.\n' >&3
    printf '  The Builder compiles them in one folder of your choice.\n\n' >&3

    default=${MAKEPAD_LOADER_ROOT:-$HOME/makepad-commercial}
    home=$(cd -P -- "$HOME" && pwd -P)
    while :; do
        printf '  Folder [%s]: ' "$default" >&3
        IFS= read -r folder <&3 || { printf '\n  Stopped. Nothing was installed.\n' >&3; return 1; }
        folder=${folder:-$default}
        case "$folder" in
            '~')   folder=$HOME ;;
            '~/'*) folder=$HOME/${folder#'~/'} ;;
            /*)    ;;
            *)     folder=$(pwd -P)/$folder ;;
        esac
        # Make it, then look at where it really is (symlinks resolved), so
        # neither / nor your home folder itself can be used by accident.
        if mkdir -p -- "$folder" 2>/dev/null && root=$(cd -P -- "$folder" && pwd -P); then
            # Only an empty folder, or one the Builder made before: the next
            # step writes files here and must not overwrite anything else.
            if [ "$root" = / ] || [ "$root" = "$home" ]; then
                printf '  Please choose a folder of its own, such as %s.\n' "$HOME/makepad-commercial" >&3
            elif [ -e "$root/makepad-builder.json" ] || [ -z "$(ls -A "$root")" ]; then
                break
            else
                printf '  %s already holds other files; please choose an empty or new folder.\n' "$root" >&3
            fi
        else
            printf '  Could not create %s; please choose another folder.\n' "$folder" >&3
        fi
    done

    # The Builder's settings. An existing file keeps your email and choices.
    [ -e "$root/makepad-builder.json" ] || printf '%s\n' @BOOTSTRAP_SHELL@ > "$root/makepad-builder.json"

    cat > "$root/check-tools.sh" <<'MAKEPAD_CHECK_TOOLS'
@PREFLIGHT_SHELL@
MAKEPAD_CHECK_TOOLS

    cat > "$root/rust-tools.sh" <<'MAKEPAD_RUST_TOOLS'
@RUST_TOOLS_SHELL@
MAKEPAD_RUST_TOOLS

    cat > "$root/bootstrap-builder.sh" <<'MAKEPAD_BOOTSTRAP'
@BUILDER_BOOTSTRAP@
MAKEPAD_BOOTSTRAP

    cat > "$root/run-builder.sh" <<'MAKEPAD_RUN'
#!/bin/sh
# Starts the Builder. It checks its setup first and opens directly when
# nothing needs attention.
set -eu
exec sh "$(cd -P -- "$(dirname -- "$0")" && pwd -P)/bootstrap-builder.sh"
MAKEPAD_RUN

    chmod 700 "$root/check-tools.sh" "$root/rust-tools.sh" "$root/bootstrap-builder.sh" "$root/run-builder.sh"
    sh "$root/bootstrap-builder.sh" <&3 >&3 2>&3
}

main
