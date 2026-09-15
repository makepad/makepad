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
exec "$builder_directory/@APP_BINARY@.bin" "$@"
