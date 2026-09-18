#!/bin/sh
# A Dock launch has no project argument. Resolve everything from this bundle,
# so the complete Builder folder can move without rewriting absolute paths.
set -eu
app_dir=$(cd -P -- "$(dirname -- "$0")" && pwd)
install_dir=$(cd -P -- "$app_dir/../../.." && pwd)
unset MAKEPAD_LOADER_EMAIL MAKEPAD_PACKAGE_DIR
has_project=0
for arg in "$@"; do
    [ "$arg" != --cwd ] || has_project=1
done
if [ "$has_project" = 0 ]; then
    project=$(cat "$install_dir/installed/@APP_BINARY@.project")
    case "$project" in /*) ;; *) project="$install_dir/$project" ;; esac
    set -- --cwd "$project" "$@"
fi
mkdir -p "$install_dir/target"
exec "$app_dir/@APP_BINARY@-bin" "$@" </dev/null >>"$install_dir/target/@APP_BINARY@-app.log" 2>&1
