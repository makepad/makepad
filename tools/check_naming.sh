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

# The widget library and its catalogue app are written clean-room against
# public design-system specs; none of those systems, their vendors or the
# tools they ship with may be named in the tree. Each name is spelled out in
# octal so this script never contains one. The catalogue crate's own name is
# the one deliberate exception, chosen by the project owner.
ds_mobile_one=$(printf '\155\141\164\145\162\151\141\154\040\144\145\163\151\147\156')
ds_mobile_two=$(printf '\155\141\164\145\162\151\141\154\040\143\157\155\160\157\156\145\156\164\163')
ds_mobile_three=$(printf '\155\141\164\145\162\151\141\154\056\151\157')
ds_desktop_one=$(printf '\146\154\165\145\156\164')
ds_desktop_two=$(printf '\167\151\156\165\151')
ds_web_one=$(printf '\141\156\164\040\144\145\163\151\147\156')
ds_web_two=$(printf '\141\156\164\144')
ds_web_three=$(printf '\143\141\162\142\157\156\040\144\145\163\151\147\156')
ds_web_four=$(printf '\155\141\156\164\151\156\145')
ds_web_five=$(printf '\163\150\141\144\143\156')
ds_vendor_one=$(printf '\147\157\157\147\154\145')
ds_vendor_two=$(printf '\155\151\143\162\157\163\157\146\164')
ds_vendor_three=$(printf '\141\160\160\154\145')

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

widget_scope="widgets/src"
if [ -d apps/storybook ]; then
    widget_scope="$widget_scope apps/storybook"
fi
if grep -rniIE --exclude-dir='target*' \
    "$ds_mobile_one|$ds_mobile_two|$ds_mobile_three|$ds_web_one|$ds_web_three" $widget_scope > "$matches"; then
    echo "forbidden design-system naming found in the widget tree:" >&2
    cat "$matches" >&2
    exit 1
fi
if grep -rniIEw --exclude-dir='target*' \
    "$ds_desktop_one|$ds_desktop_two|$ds_web_two|$ds_web_four|$ds_web_five|$ds_vendor_one|$ds_vendor_two" $widget_scope > "$matches"; then
    echo "forbidden design-system or vendor naming found in the widget tree:" >&2
    cat "$matches" >&2
    exit 1
fi

# The third vendor is also a build target: `target_vendor`/`target_os`
# lines identify a platform, not a design system, and stay allowed.
if grep -rniIEw --exclude-dir='target*' "$ds_vendor_three" $widget_scope | grep -vE 'target_(vendor|os)' > "$matches"; then
    echo "forbidden vendor naming found in the widget tree:" >&2
    cat "$matches" >&2
    exit 1
fi

if find apps/fab libs/fab libs/fab_tour libs/raytrace examples/raytrace \
    -type f -name '*.rej' -print | grep -q .; then
    echo "patch reject artifacts remain" >&2
    exit 1
fi

echo "naming gate: ok (main tree has no restricted legacy-format naming)"
