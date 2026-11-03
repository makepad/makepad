#!/bin/sh
# The private-data check: no machine or host names, node numbers, LAN addresses, user names, home
# paths or the dev machines' hardware inventory in tracked files or commit messages (AGENTS.md).
# The one entry point the git hooks, CI (makepad-private-check) and devhub all call.
#
#   scan.sh diff                 a unified diff on stdin (`git diff` shape, any context): the added
#                                lines; new and deleted files and missing `diff` lines are fine
#   scan.sh text <label>         text on stdin (a commit message): every line, hits named <label>
#   scan.sh messages             messages on stdin, each starting with a `\001<label>` line
#   scan.sh staged               the staged diff (the pre-commit hook)
#   scan.sh msg <file>           a commit message file: # lines skipped, nothing after the
#                                `commit -v` scissors line (the commit-msg hook)
#   scan.sh commits <revs...>    the diffs and messages of `git rev-list <revs...>` (the pre-push
#                                hook passes `<sha> --not --remotes`)
#   scan.sh tree                 every tracked file (CI; a Linux node's job, not the Mac's)
#
# Rules: lines of tools/private_check/rules (tracked, generic) and ~/.makepad/private_patterns
# (untracked, the exact private tokens; MAKEPAD_PRIVATE_PATTERNS names another file; missing means
# the generic rules alone). Each is `<rule> <ERE>`: a line the POSIX extended regex matches,
# case-insensitively, breaks <rule>; `and <rule> <ERE>`: ...and the line also matches this one;
# `except <rule> <ERE>`: a match of <rule> this regex matches in full is fine (placeholders).
# A real exception is a `<path or dir/> <rule> <reason>` line in tools/private_check/allow.
# local/ is never scanned. A hit prints `<file>:<line>: <rule>`, never the text, and a last line
# `private: <N> hit(s): ...` sums them up. Exit 0 clean, 1 hits, 2 a usage or git error.
set -u
here=$(cd "$(dirname "$0")" && pwd)
rules="$here/rules"
allow="$here/allow"
private="${MAKEPAD_PRIVATE_PATTERNS:-${HOME:-}/.makepad/private_patterns}"
[ -f "$private" ] || private=""
mode="${1:-}"
[ $# -gt 0 ] && shift

# `<file>\t<line>\t<text>` records on stdin; every rule applied to <text>
prog='
function load(f,   line, kind, name, re) {
    if (f == "") return
    while ((getline line < f) > 0) {
        sub(/\r$/, "", line)
        if (line ~ /^[ \t]*(#|$)/) continue
        kind = line; sub(/[ \t].*$/, "", kind)
        re = line; sub(/^[^ \t]+[ \t]+/, "", re)
        if (kind == "except" || kind == "and") {
            name = re; sub(/[ \t].*$/, "", name)
            sub(/^[^ \t]+[ \t]+/, "", re)
            if (kind == "except") ex[name] = (name in ex) ? ex[name] "|" tolower(re) : tolower(re)
            else { na2[name]++; andre[name, na2[name]] = tolower(re) }
            continue
        }
        n++; rname[n] = kind; rre[n] = tolower(re)
    }
    close(f)
}
function allowed(path, rule,   i) {
    for (i = 1; i <= nal; i++)
        if ((arule[i] == rule || arule[i] == "*") && (path == apath[i] || (apath[i] ~ /\/$/ && index(path, apath[i]) == 1)))
            return 1
    return 0
}
# true when every match of rule i in s is one its except names in full
function excused(i, s,   m, full) {
    if (!(rname[i] in ex)) return 0
    full = "^(" ex[rname[i]] ")$"
    while (match(s, rre[i])) {
        if (RLENGTH == 0) return 0
        m = substr(s, RSTART, RLENGTH)
        if (m !~ full) return 0
        s = substr(s, RSTART + RLENGTH)
    }
    return 1
}
function also(i, s,   k) {
    for (k = 1; k <= na2[rname[i]]; k++)
        if (s !~ andre[rname[i], k]) return 0
    return 1
}
BEGIN {
    load(rules); load(private)
    while ((getline line < allow) > 0) {
        sub(/\r$/, "", line)
        if (line ~ /^[ \t]*(#|$)/) continue
        split(line, w, /[ \t]+/)
        nal++; apath[nal] = w[1]; arule[nal] = w[2]
    }
    close(allow)
}
{
    t = index($0, "\t"); path = substr($0, 1, t - 1); rest = substr($0, t + 1)
    t = index(rest, "\t"); ln = substr(rest, 1, t - 1); text = tolower(substr(rest, t + 1))
    for (i = 1; i <= n; i++) {
        if (!match(text, rre[i]) || !also(i, text) || excused(i, text) || allowed(path, rname[i])) continue
        printf "%s:%s: %s\n", path, ln, rname[i]
        hits++
    }
}
END {
    if (hits > 0) {
        printf "private: %d hit(s): write \"a Linux node\", \"the Mac\", \"a 24 GB GPU\" or 192.0.2.x and repo-relative paths (AGENTS.md)\n", hits
        exit 1
    }
}'

check() { awk -v rules="$rules" -v private="$private" -v allow="$allow" "$prog"; }

# a unified diff on stdin -> one record per added line outside local/. A hunk is read by its
# @@ counts, so a body line that looks like a header (`--- x` removed, `+++ x` added) stays a body line.
added() {
    awk '
        function count(r) { return (r ~ /,/) ? substr(r, index(r, ",") + 1) + 0 : 1 }
        old > 0 || new > 0 {
            c = substr($0, 1, 1)
            if (c == "+") { if (!skip) printf "%s\t%d\t%s\n", f, ln, substr($0, 2); ln++; new-- }
            else if (c == "-") old--
            else if (c == " ") { ln++; old--; new-- }
            next
        }
        /^\+\+\+ / {
            f = substr($0, 5); sub(/\t.*$/, "", f)
            if (f ~ /^".*"$/) f = substr(f, 2, length(f) - 2)
            skip = (f == "/dev/null"); sub(/^b\//, "", f)
            if (f ~ /^local\//) skip = 1
            next
        }
        /^@@ / {
            split($0, w, " "); a = w[2]; b = w[3]
            old = count(a); new = count(b)
            sub(/^\+/, "", b); sub(/,.*$/, "", b); ln = b + 0
            next
        }
    '
}

# text on stdin -> records named <label>; with <skip>, no # lines and nothing after the scissors
text() {
    awk -v label="$1" -v skip="${2:-}" '
        skip != "" && /^# -+ >8 -+$/ { exit }
        skip == "" || !/^#/ { printf "%s\t%d\t%s\n", label, NR, $0 }
    '
}

# `\001<label>` lines start each message -> records named by its label
messages() {
    awk '/^\001/ { label = substr($0, 2); n = 0; next } { n++; printf "%s\t%d\t%s\n", label, n, $0 }'
}

case "$mode" in
diff) added | check ;;
text) text "${1:-text}" | check ;;
messages) messages | check ;;
staged) git diff --cached -U0 --no-color --no-ext-diff --no-renames --diff-filter=d | added | check ;;
msg) text "commit message" "#" < "$1" | check ;;
commits)
    git log --no-color --no-ext-diff --no-renames --diff-merges=first-parent -U0 --format= -p "$@" \
        -- . ':(exclude)local/' | added | check
    s1=$?
    git log --format='%x01message of %h%n%B' "$@" | messages | check
    s2=$?
    [ $s1 -eq 0 ] && [ $s2 -eq 0 ]
    ;;
tree)
    top=$(git rev-parse --show-toplevel) || exit 2
    cd "$top" || exit 2
    pats=$(mktemp "${TMPDIR:-/tmp}/private-check.XXXXXX") || exit 2
    trap 'rm -f -- "$pats"' EXIT
    # the rules' own patterns pick the candidate lines, the awk side decides. grep, not git grep:
    # GNU grep runs the patterns as one DFA (a Linux node: about a second of one core for the
    # whole tree; git grep: minutes of CPU). One grep at a time: parallel ones interleave lines.
    # LC_ALL=C: only a NUL makes a file binary. -d skip: a symlinked directory or a submodule.
    cat "$rules" ${private:+"$private"} | awk '!/^[ \t]*(#|$)/ && $1 != "except" && $1 != "and" { sub(/^[^ \t]+[ \t]+/, ""); print }' > "$pats"
    git ls-files -z -- . ':(exclude)local/' |
        LC_ALL=C xargs -0 grep -s -d skip -I -n -H --null -i -E -f "$pats" |
        tr '\0' '\t' |
        awk '{ t = index($0, "\t"); r = substr($0, t + 1); c = index(r, ":"); printf "%s\t%s\t%s\n", substr($0, 1, t - 1), substr(r, 1, c - 1), substr(r, c + 1) }' |
        check
    ;;
*)
    echo "usage: scan.sh diff | text <label> | messages | staged | msg <file> | commits <revs...> | tree" >&2
    exit 2
    ;;
esac
