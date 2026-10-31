#!/bin/bash
# Usage: nightly_sync.sh <nightly-checkout> [new-revision=HEAD]
# Reads every `// ported from <path> @<sha>` line under this checkout's libs/relax/.
# Use one header per upstream source when a Relax file combines multiple files.
# Paths are checkout-relative (compiler/...), or N:... for compiler-relative paths.
# Prints `git diff <sha>..<new> -- <path>` separately for each header, including
# unchanged files, and marks paths absent at the new revision as MISSING.
# Git is read-only; no fetch, checkout, or modification of either tree occurs.
# Exit 1: invalid pin/path or git failure; exit 2: invalid invocation.
set -eu
export GIT_OPTIONAL_LOCKS=0

if [ "$#" -lt 1 ] || [ "$#" -gt 2 ]; then
    echo 'Usage: nightly_sync.sh <nightly-checkout> [new-revision=HEAD]' >&2
    exit 2
fi
nightly=$1
revision=${2:-HEAD}
relax=$(cd -- "$(dirname -- "$0")/../../../libs/relax" && pwd)
new=$(git -C "$nightly" rev-parse --verify --end-of-options "$revision^{commit}")
status=0
count=0

while IFS= read -r -d '' file; do
    while IFS= read -r header; do
        if [[ ! $header =~ ^//\ ported\ from\ ([^[:space:]]+)\ @([[:xdigit:]]{7,64})[[:space:]]*$ ]]; then
            printf 'Invalid port header in %s: %s\n' "${file#"$relax/"}" "$header" >&2
            status=1
            continue
        fi
        path=${BASH_REMATCH[1]}
        pin=${BASH_REMATCH[2]}
        case "$path" in
            N:*) path=compiler/${path#N:} ;;
        esac
        case "/$path/" in
            //*|*/../*|*/./*)
                printf 'Invalid upstream path in %s: %s\n' "${file#"$relax/"}" "$path" >&2
                status=1
                continue ;;
        esac
        count=$((count + 1))
        printf '\n=== %s <- %s @%s ===\n' "${file#"$relax/"}" "$path" "$pin"
        if ! git -C "$nightly" rev-parse --verify "$pin^{commit}" >/dev/null; then
            printf 'Unavailable pinned commit: %s\n' "$pin" >&2
            status=1
            continue
        fi
        if ! git -C "$nightly" cat-file -e "$pin:$path"; then
            printf 'Missing upstream path at pinned commit: %s\n' "$path" >&2
            status=1
            continue
        fi
        if ! git -C "$nightly" cat-file -e "$new:$path" 2>/dev/null; then
            printf 'MISSING at %s: %s\n' "$new" "$path"
        fi
        printf 'git diff %s..%s -- %s\n' "$pin" "$new" "$path"
        if ! git -C "$nightly" --no-pager diff --no-ext-diff --no-textconv \
            --no-renames --color=never "$pin..$new" -- ":(literal)$path"; then
            status=1
        fi
    done < <(grep -I -E '^// ported from ' "$file" || :)
done < <(find "$relax" -type d \( -name .git -o -name target \) -prune \
    -o -type f -print0)

if [ "$count" -eq 0 ]; then
    echo 'No port headers found.'
fi
exit "$status"
