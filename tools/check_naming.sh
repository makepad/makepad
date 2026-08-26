#!/bin/sh
set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo_root"

legacy=$(printf '\142\151\155\170')
old_shell=$(printf '\144\145\163\151\147\156\063\144')
comparison=$(printf '\142\154\145\156\144\145\162')
retired_product=$(printf '\146\157\162\147\145')
old_renderer=$(printf '\160\141\164\150\164\162\141\143\145')
vendor_one=$(printf '\147\162\141\160\150\151\163\157\146\164')
vendor_two=$(printf '\141\162\143\150\151\143\141\144')

matches=$(mktemp "$repo_root/.naming-check.XXXXXX")
trap 'rm -f -- "$matches"' EXIT INT TERM

if grep -rniI --exclude-dir=local --exclude-dir='target*' --exclude-dir=.git --exclude-dir=.claude --exclude-dir=cargo_makepad --exclude='*.pem' \
    -e "$legacy" . > "$matches"; then
    echo "forbidden legacy-format naming found:" >&2
    cat "$matches" >&2
    exit 1
fi

scope="apps/fab libs/fab libs/fab_tour libs/raytrace examples/raytrace Cargo.toml makepad.splash"
if grep -rniIE --exclude-dir='target*' "$vendor_one|$vendor_two|$old_renderer|$old_shell|$comparison" $scope > "$matches"; then
    echo "forbidden vendor, comparison, or retired naming found:" >&2
    cat "$matches" >&2
    exit 1
fi
if grep -rniIEw --exclude-dir='target*' "$retired_product" $scope > "$matches"; then
    echo "forbidden retired product naming found:" >&2
    cat "$matches" >&2
    exit 1
fi

if find apps/fab libs/fab libs/fab_tour libs/raytrace examples/raytrace \
    -type f -name '*.rej' -print | grep -q .; then
    echo "patch reject artifacts remain" >&2
    exit 1
fi

echo "naming gate: ok (main tree has no restricted legacy-format naming)"
