#!/bin/sh
# Keep the invoking directory. Only the app command is exposed on PATH.
set -eu
builder_script=$0
while [ -L "$builder_script" ]; do
    builder_directory=$(cd -P -- "$(dirname -- "$builder_script")" && pwd)
    builder_link=$(readlink "$builder_script")
    case "$builder_link" in /*) builder_script=$builder_link ;; *) builder_script=$builder_directory/$builder_link ;; esac
done
builder_directory=$(cd -P -- "$(dirname -- "$builder_script")" && pwd)
unset MAKEPAD_LOADER_EMAIL MAKEPAD_PACKAGE_DIR
if [ "$#" -gt 0 ]; then
    case "$1" in -*) ;; *) set -- --cwd "$@" ;; esac
fi
if [ "$(uname -s)" = Darwin ]; then
    # The shell command opens the invoking directory; Dock launches use the
    # bundle's saved project. Keep the two entry points consistent.
    for builder_app in "$builder_directory/"*.app; do
        if [ -x "$builder_app/Contents/MacOS/@APP_BINARY@" ]; then
            builder_has_cwd=0
            for builder_arg in "$@"; do
                [ "$builder_arg" != --cwd ] || builder_has_cwd=1
            done
            if [ "$builder_has_cwd" = 0 ]; then set -- --cwd "$PWD" "$@"; fi
            exec "$builder_app/Contents/MacOS/@APP_BINARY@" "$@"
        fi
    done
fi
exec "$builder_directory/@APP_BINARY@.bin" "$@"
