#!/bin/sh
# Makepad Builder for macOS and Linux.
#
#   curl -fsSL https://makepad.nl/builder/unix | sh
#
# You can read this whole file before running it: it is one shell script and
# nothing compiled. It opens the Builder in your terminal, which asks for your
# email (the one identity; empty continues with the free apps), a folder
# (default ~/makepad-commercial) and which Rust to use, then installs Rust
# into that folder only to compile the apps. It copies itself into the folder
# as the `makepad` command:
#
#   ~/makepad-commercial/makepad             opens the Builder again
#   ~/makepad-commercial/makepad build APP   compiles an app (for coding agents)
#
# Everything the Builder downloads, builds and records stays in that folder
# (in its builder/ folder: beside `makepad` there are only the apps you
# build); Apple's developer tools or your distribution's development packages are
# the only things installed elsewhere, and it shows and asks about them first.
#
# The Builder itself is the text between the MAKEPAD_BUILDER lines below.
# Everything runs from the last line, so a download cut off halfway does
# nothing at all.

makepad_builder() {
    set -eu
    [ "$(id -u)" != 0 ] || { echo 'Run the Makepad Builder as your normal user, not as root.' >&2; return 1; }
    makepad_dir=$(mktemp -d "${TMPDIR:-/tmp}/makepad-builder.XXXXXX") || return 1
    cat > "$makepad_dir/makepad" <<'MAKEPAD_BUILDER'
#!/bin/sh
# Makepad Builder for macOS and Linux, installed as `makepad` in its folder.
#
#   ./makepad                  the Builder: licenses, apps, updates, agents
#   ./makepad build APP        compile an app after you or an agent changed it
#   ./makepad run APP          compile it when needed, then open it
#   ./makepad list             every app, its state and its source folder
#   ./makepad env              this folder's build environment, for eval
#   ./makepad agent-context    what coding agents are told about this folder
#
# One shell script and nothing compiled for itself; Rust is installed only to
# build the apps. Every run is a restart: all state is read from the folder,
# and a stopped download continues where it was. Everything stays in this
# folder, in builder/ (beside this command there are only the apps), except Apple's developer tools or your distribution's development
# packages, which it shows and asks about first.
set -eu
umask 077

service=${MAKEPAD_LOADER_SERVICE:-https://makepad.nl/api/loader}
service=${service%/}
# A personalized download names the email here; the Builder still asks.
builder_email='@EMAIL@'
case "$builder_email" in @*@) builder_email= ;; esac

case "$(uname -s)/$(uname -m)" in
    Darwin/arm64 | Darwin/aarch64) triple=aarch64-apple-darwin plat=mac ;;
    Darwin/x86_64) triple=x86_64-apple-darwin plat=mac ;;
    Linux/x86_64) triple=x86_64-unknown-linux-gnu plat=linux ;;
    Linux/aarch64 | Linux/arm64) triple=aarch64-unknown-linux-gnu plat=linux ;;
    *) printf 'The Makepad Builder supports macOS, and glibc Linux on x86_64 and ARM64.\n' >&2; exit 1 ;;
esac

self=$0
case "$self" in /*) ;; *) self="$(pwd -P)/$self" ;; esac
# The terminal UI reads keys with bash's read builtin; where dash or another
# sh started it, the same script continues under /bin/bash when there is one.
if [ "$#" = 0 ] && [ -z "${BASH_VERSION:-}" ] && [ -z "${MAKEPAD_BUILDER_SH:-}" ] && [ -x /bin/bash ] && [ -f "$self" ]; then
    MAKEPAD_BUILDER_SH=1 exec /bin/bash "$self"
fi
self_dir=$(cd -P -- "$(dirname -- "$self")" && pwd -P)
# home: the folder people see, with this command and the apps built there;
# root: the Builder's own folder in it (builder/), which holds everything
# else. Folders from before that layout are moved over on the next start.
home= root=
if [ -f "$self_dir/builder/makepad-builder.json" ] || [ -f "$self_dir/makepad-builder.json" ] || [ -f "$self_dir/makepad-loader.json" ]; then
    home=$self_dir root=$self_dir/builder
fi

# ------------------------------------------------------------ utilities ---

nl='
'
ifs_default=$(printf ' \t\n_'); ifs_default=${ifs_default%_}
# Shell pattern ranges ([a-z], [!!-~]) follow the locale's collation: in a
# UTF-8 locale they stop meaning byte ranges, and the email check refused
# every address. Collation alone is pinned to byte order; the character
# type (UTF-8 text and widths) stays the person's.
if [ -n "${LC_ALL:-}" ]; then LC_CTYPE=$LC_ALL; export LC_CTYPE; unset LC_ALL; fi
LC_COLLATE=C; export LC_COLLATE
now() { date +%s; }
short() { case "$1" in "$HOME"/*) printf '~/%s' "${1#"$HOME"/}" ;; *) printf '%s' "$1" ;; esac; }
# pad TEXT WIDTH -> $padded, without forking.
pad() { padded=$1; while [ "${#padded}" -lt "$2" ]; do padded="$padded "; done; }
sha256() {
    if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | awk '{ print $1 }'
    else shasum -a 256 "$1" | awk '{ print $1 }'; fi
}
size_of() { if [ -f "$1" ]; then wc -c < "$1" | tr -d ' '; else printf 0; fi; }
# put FILE TEXT: written through a temporary name, so readers never see half.
put() { printf '%s\n' "$2" > "$1.$$.next" && mv -f "$1.$$.next" "$1"; }
log() { [ -z "${log_file:-}" ] || printf '%s %s\n' "$(date '+%H:%M:%S')" "$*" >> "$log_file"; }
# remove_inside PATH: deletes PATH only when it lies strictly inside the
# Builder's installation folder; anything else is refused and left alone.
remove_inside() {
    [ -n "$home" ] || return 1
    case "$1" in
        "$home"/?*) case "$1" in *'/../'* | *'/..') return 1 ;; esac; rm -rf -- "$1" ;;
        *) log "refused to delete $1"; return 1 ;;
    esac
}
identifier() { case "$1" in '' | *[!A-Za-z0-9_-]*) return 1 ;; esac; [ "${#1}" -le 128 ]; }
relative() {
    case "$1" in '' | /* | */ | *//* | *[!A-Za-z0-9_/-]*) return 1 ;; esac
    case "/$1/" in */../* | */./*) return 1 ;; esac
}
hex() { case "$1" in *[!0-9a-f]*) return 1 ;; esac; [ "${#1}" = "$2" ]; }

# email TEXT -> $email_ok (lowercase) or fails: the rules of the Windows
# Builder and the service.
check_email() {
    email_ok=$(printf '%s' "$1" | sed 's/^[[:space:]]*//; s/[[:space:]]*$//' | tr 'A-Z' 'a-z')
    case "$email_ok" in *@*@* | @* | *@ | *[!!-~]* | *'<'* | *'>'* | *'"'* | *'\'*) return 1 ;; *@*) ;; *) return 1 ;; esac
    em_local=${email_ok%%@*}
    [ "${#email_ok}" -le 254 ] && [ "${#em_local}" -le 64 ]
}

# --------------------------------------------------------------- JSON ---
# One awk program reads the service's JSON: "flat" prints path<TAB>value
# per value, "items" each element of a top-level array on one line,
# "release" shell assignments for one release, "apps" one line per app.
json_awk='
function jstr(   out, r, p, c) {
    out = ""; i++
    while (i <= n) {
        r = substr(s, i, 4096)
        p = match(r, /["\\]/)
        if (p == 0) { out = out r; i += length(r); continue }
        out = out substr(r, 1, p - 1); i += p - 1
        c = substr(s, i, 1)
        if (c == "\"") { i++; return out }
        c = substr(s, i + 1, 1); i += 2
        if (c == "u") { out = out "?"; i += 4 }
        else if (c == "n" || c == "t" || c == "r" || c == "b" || c == "f") out = out " "
        else out = out c
    }
    return out
}
function jpath(   p, d) {
    p = ""
    for (d = 1; d <= depth; d++) p = p (d > 1 ? "/" : "") (kind[d] == "a" ? idx[d] : key[d])
    return p
}
function jval(v) { np++; P[np] = jpath(); V[np] = v; if (mode == "flat") print P[np] "\t" v }
function q(v) { gsub(sq, sq "\"" sq "\"" sq, v); return sq v sq }
function release_vars(   k, a, name, nrep, r, x, y, t, lines, total, pl, feat) {
    nrep = 0; pl = ""; feat = ""
    for (k = 1; k <= np; k++) {
        name = P[k]
        if (name ~ /^(id|title|release|rust|package|binary|workspace|license|public|cuda)$/) F[name] = V[k]
        else if (name ~ /^platforms\//) pl = pl " " V[k]
        else if (name ~ /^features\//) feat = feat (feat == "" ? "" : ",") V[k]
        else if (name ~ /^repositories\/[0-9]+\/[a-z0-9]+$/) {
            split(name, a, "/"); R[a[2] "," a[3]] = V[k]
            if (a[2] + 1 > nrep) nrep = a[2] + 1
        }
    }
    split("id title release rust package binary workspace license public cuda", a, " ")
    for (k = 1; k <= 10; k++) print "rel_" a[k] "=" q(F[a[k]])
    for (r = 0; r < nrep; r++) o[r] = r
    for (x = 1; x < nrep; x++)
        for (y = x; y > 0 && length(R[o[y - 1] ",path"]) > length(R[o[y] ",path"]); y--) { t = o[y]; o[y] = o[y - 1]; o[y - 1] = t }
    lines = ""; total = 0
    for (x = 0; x < nrep; x++) {
        r = o[x]
        lines = lines (x ? "\n" : "") R[r ",name"] "|" R[r ",path"] "|" R[r ",commit"] "|" R[r ",sha256"] "|" R[r ",bytes"]
        total += R[r ",bytes"]
    }
    print "rel_repos=" q(lines); print "rel_bytes=" (total + 0); print "rel_platforms=" q(pl); print "rel_features=" q(feat)
}
function app_lines(   k, a, count, x) {
    count = 0
    for (k = 1; k <= np; k++) {
        split(P[k], a, "/")
        if (a[2] == "features") A[a[1] ",features"] = A[a[1] ",features"] (A[a[1] ",features"] == "" ? "" : ",") V[k]
        else A[a[1] "," a[2]] = V[k]
        if (a[1] + 1 > count) count = a[1] + 1
    }
    for (x = 0; x < count; x++) print A[x ",id"] "|" A[x ",title"] "|" A[x ",package"] "|" A[x ",binary"] "|" A[x ",workspace"] "|" A[x ",features"] "|" A[x ",menu"]
}
BEGIN { sq = sprintf("%c", 39) }
{ s = s $0 "\n" }
END {
    n = length(s); i = 1; depth = 0; np = 0
    while (i <= n) {
        c = substr(s, i, 1)
        if (c == "{" || c == "[") {
            if (depth == 1 && kind[1] == "a") start = i
            depth++; kind[depth] = (c == "{") ? "o" : "a"; idx[depth] = 0; key[depth] = ""; want[depth] = (c == "{"); i++
        } else if (c == "}" || c == "]") {
            depth--; i++
            if (mode == "items" && depth == 1 && kind[1] == "a") print substr(s, start, i - start)
        } else if (c == ",") {
            if (kind[depth] == "a") idx[depth]++; else want[depth] = 1
            i++
        } else if (c == ":") { want[depth] = 0; i++ }
        else if (c == "\"") {
            v = jstr()
            if (kind[depth] == "o" && want[depth]) key[depth] = v; else jval(v)
        } else if (c == " " || c == "\t" || c == "\n" || c == "\r") i++
        else {
            j = i
            while (j <= n) { d = substr(s, j, 1); if (d == "," || d == "}" || d == "]" || d == " " || d == "\n" || d == "\t" || d == "\r") break; j++ }
            jval(substr(s, i, j - i)); i = j
        }
    }
    if (mode == "release") release_vars()
    if (mode == "apps") app_lines()
}'

json() { awk -v mode="$1" "$json_awk"; }

# rel_load FILE: the release's fields as rel_* variables; fails when the file
# is missing or does not describe a release this Builder can use safely.
rel_load() {
    [ -f "$1" ] || return 1
    rel_id=
    eval "$(json release < "$1")" || return 1
    rel_valid
}
rel_valid() {
    identifier "$rel_id" && identifier "$rel_release" && identifier "$rel_package" && identifier "$rel_binary" || return 1
    relative "$rel_workspace" || return 1
    case "$rel_rust" in *[!0-9.]* | *..* | .* | *.) return 1 ;; esac
    [ -n "$rel_repos" ] || return 1
    rel_makepad_path=
    while IFS='|' read -r v_name v_path v_commit v_sha v_bytes; do
        identifier "$v_name" && relative "$v_path" && hex "$v_commit" 40 && hex "$v_sha" 64 || return 1
        case "$v_bytes" in '' | *[!0-9]*) return 1 ;; esac
        [ "$v_name" != makepad ] || rel_makepad_path=$v_path
    done <<EOF
$rel_repos
EOF
    # Every repository and the workspace lie inside the Makepad checkout,
    # which the one Cargo workspace needs.
    [ -n "$rel_makepad_path" ] || return 1
    while IFS='|' read -r v_name v_path v_commit v_sha v_bytes; do
        case "$v_path/" in "$rel_makepad_path"/*) ;; *) return 1 ;; esac
    done <<EOF
$rel_repos
EOF
    case "$rel_workspace/" in "$rel_makepad_path"/*) ;; *) return 1 ;; esac
    [ -n "$rel_title" ] || rel_title=$rel_id
    [ -n "$rel_license" ] || rel_license=commercial
}
rel_supported() { case "$rel_platforms " in *" $triple "*) return 0 ;; esac; return 1; }

# The app registry: the snapshot's own copy once the Makepad source is here
# (it names packages and features for that commit); before that, the list
# below, only to show the experiments' names.
registry_fallback='wm|Makepad WM|||||
calculator|Calculator|||||other
calendar|Calendar|||||other
clock|Clock|||||other
director|Director|||||other
fab|Fab|||||other
fabric|Fabric|||||other
files|Files|||||other
finance|Finance|||||other
image|Image|||||other
mail|Mail|||||other
mixer|Mixer|||||other
notes|Notes|||||other
photos|Photos|||||other
reminders|Reminders|||||other
route|Route|||||other
score|Score|||||other
sheets|Sheets|||||other
task|Task|||||other
terminal|Terminal|||||other
weather|Weather|||||other'
registry=
load_registry() {
    registry=
    rel_load "$root/available/makepad.json" 2>/dev/null || return 0
    rel_dir
    reg_file="$rel_directory/$rel_makepad_path/tools/makepad_builder/apps.json"
    [ -f "$reg_file" ] || return 0
    registry=$(json apps < "$reg_file")
}
# The experiments list: Makepad WM first, then the registry's "other" apps.
free_apps() {
    reg=${registry:-$registry_fallback}
    printf '%s\n' "$reg" | awk -F'|' '$1 == "wm" { print $1 "|" $2 }'
    printf '%s\n' "$reg" | awk -F'|' '$7 == "other" { print $1 "|" $2 }'
}
is_free() { printf '%s\n' "${registry:-$registry_fallback}" | grep -q "^$1|"; }

# --------------------------------------------------------------- network ---
# curl without redirects; HTTPS only (plain HTTP only to this computer, for
# a local test service). The email travels in a header file, never in a
# process argument.
curl_safe() {
    case "$1" in
        http://127.0.0.1:* | http://localhost:*) shift; curl --proto '=http' --max-redirs 0 "$@" ;;
        *) shift; curl --proto '=https' --tlsv1.2 --max-redirs 0 "$@" ;;
    esac
}
# The header file for the email; created once per run, readable only by you.
email_headers() {
    email_header_file="$scratch/email-headers"
    printf 'X-Makepad-Email: %s\r\nCache-Control: no-store\r\n' "$email" > "$email_header_file"
}
# http_get URL FILE [HEADERFILE]: body to FILE; prints the HTTP status.
http_get() {
    if [ -n "${3:-}" ]; then
        curl_safe "$1" -sS -o "$2" -w '%{http_code}' -H @"$3" "$1" 2>>"${log_file:-/dev/null}" || printf 000
    else
        curl_safe "$1" -sS -o "$2" -w '%{http_code}' "$1" 2>>"${log_file:-/dev/null}" || printf 000
    fi
}
# The service's {"error": "..."} as one line.
http_error() { json flat < "$1" 2>/dev/null | awk -F'\t' '$1 == "error" { print $2; exit }'; }

# The newest public Makepad release (free apps and the pinned Rust) from the
# public catalog; kept in available/makepad.json.
fetch_public() {
    code=$(http_get "$service/public/catalog" "$scratch/public.json")
    [ "$code" = 200 ] || { public_error="public catalog: HTTP $code $(http_error "$scratch/public.json")"; return 1; }
    json items < "$scratch/public.json" > "$scratch/public.items"
    while IFS= read -r item; do
        printf '%s\n' "$item" > "$scratch/public.one"
        if rel_load "$scratch/public.one" && [ "$rel_public" = true ]; then
            mkdir -p "$root/available"
            mv -f "$scratch/public.one" "$root/available/makepad.json"
            return 0
        fi
    done < "$scratch/public.items"
    public_error='The public Makepad source is unavailable.'
    return 1
}

# The licensed apps of $email: available/<id>.json for each, $licensed_ids
# in catalog order. lic_state: none (no email), ok, rejected, error.
check_licenses() {
    licensed_ids= lic_error=
    if [ -z "$email" ]; then lic_state=none; return 0; fi
    email_headers
    code=$(http_get "$service/catalog" "$scratch/catalog.json" "$email_header_file")
    case "$code" in
        200) ;;
        401 | 403) lic_state=rejected lic_error=$(http_error "$scratch/catalog.json"); return 1 ;;
        *) lic_state=error lic_error="HTTP $code $(http_error "$scratch/catalog.json")"; [ "$code" != 000 ] || lic_error='the license service could not be reached'; return 1 ;;
    esac
    json items < "$scratch/catalog.json" > "$scratch/catalog.items"
    [ -z "$root" ] || mkdir -p "$root/available"
    while IFS= read -r item; do
        printf '%s\n' "$item" > "$scratch/catalog.one"
        rel_load "$scratch/catalog.one" || continue
        [ "$rel_public" != true ] || continue
        licensed_ids="$licensed_ids $rel_id"
        [ -z "$root" ] || cp "$scratch/catalog.one" "$root/available/$rel_id.json"
    done < "$scratch/catalog.items"
    lic_state=ok
    put "$scratch/licensed" "$licensed_ids"
    return 0
}
license_count() { set -- $licensed_ids; license_count=$#; }

# ------------------------------------------------------ releases on disk ---
# The release file behind an app's row: cached (available/), installed, or
# the public release projected onto the app's registry entry.
app_release_file() {
    for arf in "$root/available/$1.json" "$root/installed/$1.json"; do
        [ -f "$arf" ] && { release_file=$arf; return 0; }
    done
    is_free "$1" || return 1
    project_public "$1" "$root/tmp/projected-$1.json" || return 1
    release_file="$root/tmp/projected-$1.json"
}
# project_public APP FILE: the public Makepad release as app APP (its
# package, binary, workspace and features from the registry).
project_public() {
    rel_load "$root/available/makepad.json" || return 1
    line=$(printf '%s\n' "${registry:-}" | awk -F'|' -v id="$1" '$1 == id { print; exit }')
    [ -n "$line" ] || return 1
    IFS='|' read -r d_id d_title d_package d_binary d_workspace d_features d_menu <<EOF
$line
EOF
    [ -n "$d_package" ] || return 1
    mp=$(printf '%s\n' "$rel_repos" | awk -F'|' '$1 == "makepad"')
    IFS='|' read -r m_name m_path m_commit m_sha m_bytes <<EOF
$mp
EOF
    features=
    if [ -n "$d_features" ]; then features="\"$(printf '%s' "$d_features" | sed 's/,/","/g')\""; fi
    platforms=
    for p in $rel_platforms; do platforms="$platforms${platforms:+,}\"$p\""; done
    title=$(printf '%s' "$d_title" | tr -d '"\\')
    mkdir -p "$(dirname "$2")"
    put "$2" "{\"id\":\"$d_id\",\"title\":\"$title\",\"release\":\"$rel_release\",\"rust\":\"$rel_rust\",\"package\":\"$d_package\",\"binary\":\"$d_binary\",\"workspace\":\"$d_workspace\",\"platforms\":[$platforms],\"cuda\":false,\"public\":true,\"features\":[$features],\"repositories\":[{\"name\":\"makepad\",\"path\":\"$m_path\",\"commit\":\"$m_commit\",\"bytes\":$m_bytes,\"sha256\":\"$m_sha\"}]}"
}

# repo_installed DIR NAME PATH COMMIT: the receipt names the commit and the
# checkout is there. The commit identifies the tree, whichever pack it came in.
repo_installed() {
    [ -d "$1/$3/.git" ] || return 1
    ri_commit=
    read -r ri_commit ri_rest < "$1/.builder-repositories/$2" 2>/dev/null || :
    [ "$ri_commit" = "$4" ]
}
# rel_dir: the snapshot folder this release builds from, as the Windows
# Builder picks it: sources/<release> when it exists, else an existing
# snapshot holding the same commits (so their build output is shared),
# preferring one an app was built from; else a new sources/<release>.
rel_dir() {
    rel_directory="$root/sources/$rel_release"
    [ ! -e "$rel_directory" ] || { rel_label=$rel_release; return 0; }
    rd_best=
    for rd_cand in "$root"/sources/*; do
        [ -d "$rd_cand" ] || continue
        rd_held=0 rd_usable=1
        while IFS='|' read -r v_name v_path v_commit v_sha v_bytes; do
            if repo_installed "$rd_cand" "$v_name" "$v_path" "$v_commit"; then rd_held=$((rd_held + 1))
            elif [ -e "$rd_cand/$v_path" ]; then rd_usable=0; break; fi
        done <<EOF
$rel_repos
EOF
        [ "$rd_usable" = 1 ] && [ "$rd_held" -gt 0 ] || continue
        rd_built=0; [ ! -f "$rd_cand/.builder-built" ] || rd_built=1
        rd_best="$rd_best$rd_built $((1000 + rd_held)) ${rd_cand##*/}|$rd_cand
"
    done
    if [ -n "$rd_best" ]; then
        rel_directory=$(printf '%s' "$rd_best" | sort | tail -n 1 | cut -d'|' -f2-)
    fi
    rel_label=${rel_directory##*/}
}
rel_installed() {
    while IFS='|' read -r v_name v_path v_commit v_sha v_bytes; do
        repo_installed "$rel_directory" "$v_name" "$v_path" "$v_commit" || return 1
    done <<EOF
$rel_repos
EOF
    [ -f "$rel_directory/$rel_workspace/Cargo.toml" ]
}
rel_target() { rel_target_dir="$root/target/sources/$rel_label"; }

# app_state ID: new, partial, compile, update, ready or merge, from the
# folder alone (the state machine of the Windows Builder).
app_state() {
    as_id=$1
    if [ -f "$root/changes/$as_id.merge" ]; then state=merge; return; fi
    app_release_file "$as_id" && rel_load "$release_file" || { state=new; return; }
    rel_dir
    as_bin="$root/$rel_binary.bin"
    as_installed=
    if [ -f "$root/installed/$as_id.json" ]; then
        as_installed=$(json flat < "$root/installed/$as_id.json" | awk -F'\t' '$1 == "release" { print $2; exit }')
    fi
    if [ "$as_installed" = "$rel_release" ] && [ -f "$as_bin" ]; then
        if sources_changed; then state=compile; else state=ready; fi
    elif rel_installed; then state=compile
    elif [ -n "$as_installed" ]; then state=update
    elif [ -n "$(ls -A "$rel_directory/.builder-repositories" 2>/dev/null)" ] || ls "$root"/cache/*.pack.part* >/dev/null 2>&1; then state=partial
    else state=new; fi
}
# Whether a source file Cargo used for the built app (its .d file) is newer
# than the app: an agent or the person edited it, so it compiles again.
sources_changed() {
    rel_target
    sc_deps="$rel_target_dir/release/$rel_binary.d"
    [ -f "$sc_deps" ] || return 1
    # Its first line: "target: source source ...", spaces in paths as "\ ".
    sc_list=$(awk 'NR == 1 { sub(/^[^:]*: /, ""); gsub(/\\ /, "\001"); n = split($0, a, " "); for (i = 1; i <= n; i++) { gsub(/\001/, " ", a[i]); print a[i] } exit }' "$sc_deps")
    while IFS= read -r sc_file; do
        [ -n "$sc_file" ] || continue
        [ -e "$sc_file" ] || return 0
        [ ! "$sc_file" -nt "$as_bin" ] || return 0
    done <<EOF
$sc_list
EOF
    return 1
}

# ------------------------------------------------------------ compiler ---
# The recorded choice (selected-rust): "private", or the sysroot of your own
# Rust. rust-check below validates it on every start and before each build.
rust_version() {
    rv=
    if rel_load "$root/available/makepad.json" 2>/dev/null; then rv=$rel_rust; fi
    rust_version=$rv
}
private_rust_dir() { private_rust="$root/toolchain/rust/$1-$triple"; }
private_rust_ready() {
    private_rust_dir "$1"
    [ "$(cat "$private_rust/.toolchain-version" 2>/dev/null || :)" = "$1 $triple" ] &&
        [ -x "$private_rust/bin/rustc" ] && [ -x "$private_rust/bin/cargo" ]
}
recorded_rust() { selected_rust=$(cat "$root/selected-rust" 2>/dev/null || :); }
record_rust() { put "$root/selected-rust" "$1"; }
# rust_ready VERSION: sets rust_sysroot when the chosen Rust can build.
rust_ready() {
    recorded_rust
    rust_sysroot=
    case "$selected_rust" in
        /*) rust_check --validate "$1" "$selected_rust" && return 0; return 1 ;;
        *) private_rust_ready "$1" && rust_sysroot=$private_rust ;;
    esac
}
# rust_check --probe MIN | --validate MIN SYSROOT: read-only. Prints nothing;
# sets rust_sysroot, or rust_reason. Only the toolchain's own binaries run,
# so no rustup proxy can download or switch anything.
rust_check() {
    rust_reason= rust_sysroot=
    if [ "$1" = --probe ]; then
        rc_toolchain=$(cd / && unset RUSTUP_TOOLCHAIN && RUSTUP_AUTO_INSTALL=0 rustc --print sysroot 2>/dev/null) || { rust_reason='No installed Rust was found on PATH.'; return 1; }
        [ -n "$rc_toolchain" ] || { rust_reason='No installed Rust was found on PATH.'; return 1; }
    else
        rc_toolchain=$3
    fi
    rc_min=$2
    case "$rc_toolchain" in /*) ;; *) rust_reason='The recorded Rust location is not an absolute path.'; return 1 ;; esac
    [ -d "$rc_toolchain" ] || { rust_reason="Rust is no longer installed at $rc_toolchain."; return 1; }
    rc_toolchain=$(cd -P -- "$rc_toolchain" && pwd)
    [ -x "$rc_toolchain/bin/rustc" ] || { rust_reason="No rustc in $rc_toolchain/bin."; return 1; }
    rc_meta=$("$rc_toolchain/bin/rustc" --version --verbose 2>/dev/null) || { rust_reason="rustc in $rc_toolchain/bin does not run."; return 1; }
    rc_version=$(printf '%s\n' "$rc_meta" | awk '$1 == "release:" { print $2 }')
    rc_host=$(printf '%s\n' "$rc_meta" | awk '$1 == "host:" { print $2 }')
    [ "$rc_host" = "$triple" ] || { rust_reason="Installed Rust is built for ${rc_host:-an unknown host}; this Builder needs $triple."; return 1; }
    rc_status=0
    version_at_least "$rc_version" "$rc_min" || rc_status=$?
    case "$rc_status" in
        0) ;;
        1) rust_reason="Installed Rust $rc_version is not a stable release; the Builder needs $rc_min or newer."; return 1 ;;
        *) rust_reason="Installed Rust $rc_version is older than $rc_min."; return 1 ;;
    esac
    [ -x "$rc_toolchain/bin/cargo" ] || { rust_reason='Installed Rust has no cargo beside rustc.'; return 1; }
    rc_cargo=$("$rc_toolchain/bin/cargo" --version 2>/dev/null | awk '$1 == "cargo" { print $2 }') || :
    version_at_least "${rc_cargo:-none}" "$rc_min" || { rust_reason="Installed cargo ${rc_cargo:-is unusable and} is older than $rc_min."; return 1; }
    [ -d "$rc_toolchain/lib/rustlib/$triple/lib" ] || { rust_reason="Installed Rust has no standard library for $triple."; return 1; }
    if [ "$plat" = mac ]; then
        # Release builds strip through rustc's bundled rust-objcopy; report a
        # copy that cannot load its LLVM library, never repair it.
        "$rc_toolchain/lib/rustlib/$triple/bin/rust-objcopy" --version >/dev/null 2>&1 || { rust_reason="Installed Rust's rust-objcopy does not run; release builds could not be stripped."; return 1; }
    fi
    rust_sysroot=$rc_toolchain
}
# version_at_least ACTUAL MINIMUM: 0 when newer or equal, 1 when not a
# stable x.y.z, 2 when older. Components compare as numbers.
version_at_least() {
    awk -v actual="$1" -v minimum="$2" 'BEGIN {
        if (actual !~ /^[0-9]+[.][0-9]+[.][0-9]+$/) exit 1
        split(actual, a, "."); split(minimum, b, ".")
        for (i = 1; i <= 3; i++) { if (a[i] + 0 > b[i] + 0) exit 0; if (a[i] + 0 < b[i] + 0) exit 2 }
        exit 0
    }'
}
# macOS: rust-objcopy finds LLVM beside rustlib's host tools. A relative
# link keeps the folder movable.
prepare_host_tools() {
    [ "$plat" = mac ] || return 0
    pht_lib="$1/lib/rustlib/$triple/lib"
    if [ -f "$1/lib/libLLVM.dylib" ] && [ ! -e "$pht_lib/libLLVM.dylib" ]; then
        mkdir -p "$pht_lib"
        ln -s ../../../libLLVM.dylib "$pht_lib/libLLVM.dylib"
    fi
}

# System tools: Apple's command line developer tools on macOS, the
# distribution's development packages on Linux. Read-only; tools_problem is
# empty when ready, else missing, license or unsupported.
tools_check() {
    tools_problem=
    if [ "$plat" = mac ]; then
        tc_dev=${DEVELOPER_DIR:-$(/usr/bin/xcode-select -p 2>/dev/null || :)}
        if [ -z "$tc_dev" ] || [ ! -d "$tc_dev" ]; then tools_problem=missing; return 1; fi
        for tc_probe in 'clang --version' '--show-sdk-path' '--find ld'; do
            # shellcheck disable=SC2086 # the probe words are fixed above
            if ! tc_detail=$(/usr/bin/xcrun --sdk macosx $tc_probe 2>&1); then
                case "$tc_detail" in *license*) tools_problem=license ;; *) tools_problem=missing ;; esac
                return 1
            fi
        done
        git --version >/dev/null 2>&1 || { tools_problem=missing; return 1; }
    else
        getconf GNU_LIBC_VERSION >/dev/null 2>&1 || { tools_problem=unsupported; return 1; }
        for tc_tool in cc c++ make cmake git curl tar gzip pkg-config; do
            command -v "$tc_tool" >/dev/null 2>&1 || { tools_problem=missing; return 1; }
        done
        pkg-config --exists openssl x11 xcursor xkbcommon xrandr xi xinerama alsa libpulse \
            wayland-client egl gl glx libdrm gbm || { tools_problem=missing; return 1; }
    fi
    command -v curl >/dev/null 2>&1 || { tools_problem=missing; return 1; }
    return 0
}
# Linux: the one command that installs the development packages, shown
# before it runs. Sets packages_cmd (without sudo) and distro_name.
linux_packages() {
    ID= ID_LIKE= PRETTY_NAME=
    # shellcheck disable=SC1091
    [ ! -r /etc/os-release ] || . /etc/os-release
    distro_name=${PRETTY_NAME:-your distribution}
    case " $ID $ID_LIKE " in
        *debian* | *ubuntu*) packages_cmd='apt-get install -y --no-install-recommends build-essential clang cmake pkg-config git curl ca-certificates tar gzip libssl-dev libx11-dev libxcursor-dev libxkbcommon-dev libxrandr-dev libxi-dev libxinerama-dev libasound2-dev libpulse-dev libwayland-dev wayland-protocols libegl-dev libgl-dev libglx-dev libdrm-dev libgbm-dev' ;;
        *fedora* | *rhel* | *centos*) packages_cmd='dnf install -y gcc gcc-c++ make clang cmake pkgconf-pkg-config git curl ca-certificates tar gzip openssl-devel libX11-devel libXcursor-devel libxkbcommon-devel libXrandr-devel libXi-devel libXinerama-devel alsa-lib-devel pulseaudio-libs-devel wayland-devel wayland-protocols-devel libglvnd-devel libdrm-devel mesa-libgbm-devel' ;;
        *arch*) packages_cmd='pacman -S --needed --noconfirm base-devel clang cmake pkgconf git curl ca-certificates tar gzip openssl libx11 libxcursor libxkbcommon libxrandr libxi libxinerama alsa-lib libpulse wayland wayland-protocols libglvnd mesa libdrm' ;;
        *suse*) packages_cmd='zypper --non-interactive install gcc gcc-c++ make clang cmake pkg-config git curl ca-certificates tar gzip libopenssl-devel libX11-devel libXcursor-devel libxkbcommon-devel libXrandr-devel libXi-devel libXinerama-devel alsa-devel libpulse-devel wayland-devel wayland-protocols-devel libglvnd-devel libdrm-devel Mesa-libgbm-devel' ;;
        *) packages_cmd= ; return 1 ;;
    esac
}

# ------------------------------------------------------- build environment ---
# rustc's parallel frontend (-Zthreads, allowed on the stable compiler by
# RUSTC_BOOTSTRAP=1) compiles about a third faster; the Windows Builder and
# this script use it the same way. The thread count lives in rustc-threads,
# written once per folder: the processor count, at most 8. 1 means one
# thread: the parallel frontend failed here once, and the file says why.
# Every cargo run (the Builder's builds, `makepad build`, agent shells,
# `makepad env`) gets the same flags, so the shared target stays fresh.
rustc_threads() {
    rustc_threads=
    [ ! -f "$root/rustc-threads" ] || read -r rustc_threads rt_rest < "$root/rustc-threads" || :
    case "$rustc_threads" in '' | *[!0-9]* | 0) rustc_threads= ;; esac
    if [ -z "$rustc_threads" ]; then
        rustc_threads=$(nproc 2>/dev/null || getconf _NPROCESSORS_ONLN 2>/dev/null || sysctl -n hw.ncpu 2>/dev/null || echo 1)
        case "$rustc_threads" in '' | *[!0-9]* | 0) rustc_threads=1 ;; esac
        [ "$rustc_threads" -le 8 ] || rustc_threads=8
        put "$root/rustc-threads" "$rustc_threads"
    fi
}
cargo_env() { # cargo_env LABEL APP [THREADS]: exports into the current (sub)shell
    # Inherited overrides would move Cargo's output or change its flags from
    # one session to the next, rebuilding everything.
    unset MAKEPAD_LOADER_EMAIL RUSTUP_TOOLCHAIN CARGO_ENCODED_RUSTFLAGS RUSTC_WRAPPER \
        RUSTC_WORKSPACE_WRAPPER CARGO_INCREMENTAL CARGO_BUILD_TARGET CARGO_BUILD_TARGET_DIR \
        CARGO_BUILD_RUSTFLAGS CARGO_BUILD_RUSTDOCFLAGS CARGO_BUILD_DEP_INFO_BASEDIR \
        CARGO_TARGET_APPLIES_TO_HOST CARGO_UNSTABLE_BUILD_STD RUSTDOC 2>/dev/null || :
    for ce_var in $(env | sed -n 's/^\(CARGO_PROFILE_[A-Za-z0-9_]*\)=.*/\1/p'); do unset "$ce_var"; done
    mkdir -p "$root/cargo-home" "$root/tmp" "$root/target/sources/$1"
    export CARGO_HOME="$root/cargo-home"
    export CARGO_TARGET_DIR="$root/target/sources/$1"
    export RUSTC="$rust_sysroot/bin/rustc" CARGO="$rust_sysroot/bin/cargo"
    export TMPDIR="$root/tmp" TMP="$root/tmp" TEMP="$root/tmp"
    export RUSTUP_HOME="$root/rustup-home" RUSTUP_AUTO_INSTALL=0 RUSTUP_TOOLCHAIN=
    export MAKEPAD_LOADER_EMAIL= MAKEPAD_PACKAGE_DIR=. MAKEPAD_BUILDER_APP="$2"
    export CUDA_PATH= CUDA_HOME= CUDACXX= MAKEPAD_GGML_NO_CUDA=1 MAKEPAD_GGML_REQUIRE_CUDA=0
    # Pinned: the shell's RUSTFLAGS would change every fingerprint in the
    # shared target.
    if [ -n "${3:-}" ]; then ce_threads=$3; else rustc_threads; ce_threads=$rustc_threads; fi
    if [ "$ce_threads" -gt 1 ]; then export RUSTC_BOOTSTRAP=1 RUSTFLAGS="-Zthreads=$ce_threads"
    else export RUSTC_BOOTSTRAP= RUSTFLAGS=; fi
    if [ -x "$rust_sysroot/bin/rustdoc" ]; then export RUSTDOC="$rust_sysroot/bin/rustdoc"; fi
    export PATH="$rust_sysroot/bin:$root/cargo-home/bin:/usr/bin:/bin:/usr/sbin:/sbin:$PATH"
}

# prepare_target: the snapshot's target directory. A folder from before the
# per-snapshot layout has Cargo's output directly in target/; the snapshot
# built last adopts it, so its apps do not compile again.
prepare_target() {
    rel_target
    if [ ! -e "$rel_target_dir" ] && { [ -d "$root/target/release" ] || [ -f "$root/target/.rustc_info.json" ]; }; then
        pt_newest=$(ls -t "$root"/sources/*/.builder-built 2>/dev/null | head -n 1)
        if [ -z "$pt_newest" ] || [ "${pt_newest%/.builder-built}" = "$rel_directory" ]; then
            mkdir -p "$rel_target_dir"
            for pt_entry in "$root"/target/* "$root"/target/.[!.]*; do
                [ -e "$pt_entry" ] || continue
                [ "${pt_entry##*/}" != sources ] || continue
                mv "$pt_entry" "$rel_target_dir/"
            done
        fi
    fi
    mkdir -p "$rel_target_dir"
}

# ------------------------------------------------------------ downloads ---
# Byte ranges over parallel connections for large files, when the host
# offers them; every curl of this run shares one cap of $max_connections.
max_connections=8
slot_take() {
    while :; do
        st_i=0
        while [ "$st_i" -lt "$max_connections" ]; do
            if mkdir "$slots/$st_i" 2>/dev/null; then slot=$st_i; return 0; fi
            st_i=$((st_i + 1))
        done
        sleep 0.1
    done
}
slot_give() { rmdir "$slots/$1" 2>/dev/null || :; }

# fetch URL FILE SIZE RANGES [CURL-ARGS...]: downloads FILE in the background.
# RANGES=yes splits a known SIZE into parts FILE.part0..N (up to 8, at least
# 8 MB each), each resumed from what an earlier run left; otherwise one
# connection into FILE.part0, resumed when the host allows. FILE.status
# reads "running" until done, then 0 or 1. Sets fetch_parts.
fetch() {
    f_url=$1 f_file=$2 f_size=$3 f_ranges=$4; shift 4
    fetch_parts=1
    if [ "$f_ranges" = yes ] && [ "$f_size" -gt 16777216 ]; then
        fetch_parts=$((f_size / 8388608))
        [ "$fetch_parts" -le 8 ] || fetch_parts=8
    fi
    printf 'running\n' > "$f_file.status"
    (
        f_pids=
        f_part_size=$(( (f_size + fetch_parts - 1) / fetch_parts ))
        f_i=0
        while [ "$f_i" -lt "$fetch_parts" ]; do
            f_part="$f_file.part$f_i"
            if [ "$fetch_parts" = 1 ]; then
                (
                    slot_take
                    trap 'slot_give $slot' EXIT
                    if [ -s "$f_part" ] && [ "$f_size" -gt 0 ] && [ "$(size_of "$f_part")" -ge "$f_size" ]; then exit 0; fi
                    if [ -s "$f_part" ] && curl_safe "$f_url" -sS -f -C - -o "$f_part" "$@" "$f_url" 2>>"$log_file"; then exit 0; fi
                    # No resume from this host: from the start. Without -f
                    # the service's error text lands in the part, for the
                    # step's reason; the HTTP status says which it is.
                    : > "$f_part"
                    f_code=$(curl_safe "$f_url" -sS -o "$f_part" -w '%{http_code}' "$@" "$f_url" 2>>"$log_file") || f_code=000
                    printf '%s\n' "$f_code" > "$f_file.code"
                    case "$f_code" in 200 | 206) exit 0 ;; *) exit 1 ;; esac
                ) &
                f_pids="$f_pids $!"
            else
                f_from=$((f_i * f_part_size))
                f_to=$((f_from + f_part_size - 1))
                [ "$f_to" -lt "$f_size" ] || f_to=$((f_size - 1))
                f_have=$(size_of "$f_part")
                if [ "$f_have" -lt $((f_to - f_from + 1)) ]; then
                    (
                        slot_take
                        trap 'slot_give $slot' EXIT
                        # A part cut off earlier continues after its last byte.
                        curl_safe "$f_url" -sS -f --range "$((f_from + f_have))-$f_to" "$@" "$f_url" >> "$f_part" 2>>"$log_file"
                    ) &
                    f_pids="$f_pids $!"
                fi
            fi
            f_i=$((f_i + 1))
        done
        f_status=0
        for f_pid in $f_pids; do wait "$f_pid" || f_status=1; done
        printf '%s\n' "$f_status" > "$f_file.status"
    ) &
}
# fetch_bytes FILE PARTS: bytes of FILE's parts so far.
fetch_bytes() {
    fb_sum=0 fb_i=0
    while [ "$fb_i" -lt "$2" ]; do
        [ ! -f "$1.part$fb_i" ] || fb_sum=$((fb_sum + $(size_of "$1.part$fb_i")))
        fb_i=$((fb_i + 1))
    done
    fetch_bytes=$fb_sum
}
# fetch_join FILE PARTS SIZE: the parts become FILE.
fetch_join() {
    if [ "$2" = 1 ]; then mv -f "$1.part0" "$1"; else
        fj_i=0
        : > "$1.joined"
        while [ "$fj_i" -lt "$2" ]; do cat "$1.part$fj_i" >> "$1.joined"; fj_i=$((fj_i + 1)); done
        [ "$3" = 0 ] || [ "$(size_of "$1.joined")" = "$3" ] || return 1
        mv -f "$1.joined" "$1"
        fj_i=0
        while [ "$fj_i" -lt "$2" ]; do rm -f "$1.part$fj_i"; fj_i=$((fj_i + 1)); done
    fi
    rm -f "$1.status" "$1.code"
}
# head_size URL: Content-Length when the host offers byte ranges, else 0.
head_size() {
    hs=$(curl_safe "$1" -sS -I "$1" 2>/dev/null | tr -d '\r') || hs=
    if printf '%s\n' "$hs" | grep -qi '^accept-ranges: *bytes'; then
        printf '%s\n' "$hs" | awk 'tolower($1) == "content-length:" { n = $2 } END { print n + 0 }'
    else
        printf 0
    fi
}

# ----------------------------------------------------------------- steps ---
# A work page lists steps, one per line: ○ next, ● now with its bar, ✓ done
# with a summary, ✗ failed with the reason, ◌ held. Each step's state is one
# line in a file, written by the step itself (background jobs included) and
# drawn about ten times a second:
#   now|DONE|TOTAL|UNIT|BYTES|DOING   done|SUMMARY   failed|REASON   held|REASON
step_key() { step_key=$(printf '%s' "$1" | tr -c 'A-Za-z0-9' '_'); }
step() { # step LABEL LINE
    step_key "$1"
    put "$steps_dir/$step_key" "$2"
}
step_fail() { step "$1" "failed|$2"; log "$1 failed: $2"; }

# ------------------------------------------------------------ Rust job ---
# rustc, rust-std and cargo download side by side, the large ones in byte
# ranges, each checked against its SHA-256 and unpacked in the background as
# soon as it is in. One bar counts every archive once downloaded and once
# unpacked. A stopped run keeps its parts and staging folder.
job_rust() { # job_rust LABEL VERSION
    jr_label=$1 jr_version=$2
    jr_started=$(now)
    private_rust_dir "$jr_version"
    mkdir -p "$root/cache" "$root/toolchain/rust" "$root/tmp"
    step "$jr_label" "now|0|0|MB|0|reading the manifest"
    jr_manifest="$root/cache/channel-rust-$jr_version.toml"
    if [ ! -s "$jr_manifest" ]; then
        curl_safe "https://static.rust-lang.org" -sS -f -o "$jr_manifest.part" "https://static.rust-lang.org/dist/channel-rust-$jr_version.toml" 2>>"$log_file" ||
            { step_fail "$jr_label" 'Could not download the Rust manifest'; return 1; }
        mv -f "$jr_manifest.part" "$jr_manifest"
    fi
    jr_staging="$root/toolchain/rust/.staging-$jr_version-$triple"
    mkdir -p "$jr_staging"
    jr_n=0
    for jr_component in rustc rust-std cargo; do
        jr_n=$((jr_n + 1))
        jr_section="[pkg.$jr_component.target.$triple]"
        jr_url=$(awk -v section="$jr_section" -v key=url '$0 == section { inside = 1; next } /^\[/ { inside = 0 } inside && $1 == key && $2 == "=" { gsub(/"/, "", $3); print $3; exit }' "$jr_manifest")
        jr_hash=$(awk -v section="$jr_section" -v key=hash '$0 == section { inside = 1; next } /^\[/ { inside = 0 } inside && $1 == key && $2 == "=" { gsub(/"/, "", $3); print $3; exit }' "$jr_manifest")
        case "$jr_url" in https://static.rust-lang.org/dist/*.tar.gz) ;; *) step_fail "$jr_label" "Unexpected Rust download address for $jr_component"; return 1 ;; esac
        hex "$jr_hash" 64 || { step_fail "$jr_label" "Invalid Rust checksum for $jr_component"; return 1; }
        jr_archive="$root/cache/$jr_hash.tar.gz"
        eval "jr_url_$jr_n=\$jr_url jr_hash_$jr_n=\$jr_hash jr_archive_$jr_n=\$jr_archive jr_name_$jr_n=\$jr_component"
        if [ -f "$jr_archive" ] && [ "$(sha256 "$jr_archive")" = "$jr_hash" ]; then
            eval "jr_size_$jr_n=$(size_of "$jr_archive") jr_state_$jr_n=ready"
        else
            jr_size=$(head_size "$jr_url")
            fetch "$jr_url" "$jr_archive" "$jr_size" yes
            eval "jr_size_$jr_n=\$jr_size jr_parts_$jr_n=\$fetch_parts jr_state_$jr_n=down"
        fi
    done
    while :; do
        jr_busy=0 jr_total=0 jr_done=0 jr_down=0 jr_unpack=0
        jr_n=0
        while [ "$jr_n" -lt 3 ]; do
            jr_n=$((jr_n + 1))
            eval "jr_state=\$jr_state_$jr_n jr_size=\$jr_size_$jr_n jr_archive=\$jr_archive_$jr_n jr_hash=\$jr_hash_$jr_n jr_url=\$jr_url_$jr_n jr_component=\$jr_name_$jr_n"
            case "$jr_state" in
                down)
                    eval "jr_parts=\$jr_parts_$jr_n"
                    case "$(cat "$jr_archive.status" 2>/dev/null || echo running)" in
                        running) jr_down=$((jr_down + 1)); fetch_bytes "$jr_archive" "$jr_parts"; jr_done=$((jr_done + fetch_bytes)) ;;
                        0)
                            fetch_join "$jr_archive" "$jr_parts" "$jr_size" || { step_fail "$jr_label" "Download failed: $jr_component"; return 1; }
                            [ "$(sha256 "$jr_archive")" = "$jr_hash" ] || { rm -f "$jr_archive"; step_fail "$jr_label" "The $jr_component download does not match its checksum"; return 1; }
                            jr_size=$(size_of "$jr_archive")
                            eval "jr_size_$jr_n=$jr_size jr_state_$jr_n=ready"
                            jr_done=$((jr_done + jr_size)) ;;
                        *) step_fail "$jr_label" "Download failed: $jr_component (see builder.log)"; return 1 ;;
                    esac ;;
                ready)
                    # An official archive holds <name>/<component>/{bin,lib,...};
                    # only that inner folder is unpacked. cargo's manifest.in is kept.
                    jr_top=$(basename "$jr_url" .tar.gz)
                    jr_inner=$jr_component
                    [ "$jr_component" != rust-std ] || jr_inner=rust-std-$triple
                    jr_keep=
                    [ "$jr_component" = cargo ] || jr_keep="--exclude=$jr_top/$jr_inner/manifest.in"
                    printf 'running\n' > "$root/tmp/rust-$jr_n.unpack"
                    (
                        jr_status=0
                        # shellcheck disable=SC2086 # jr_keep is one word or none
                        tar -xzf "$jr_archive" -C "$jr_staging" --strip-components=2 $jr_keep "$jr_top/$jr_inner" 2>>"$log_file" || jr_status=1
                        printf '%s\n' "$jr_status" > "$root/tmp/rust-$jr_n.unpack"
                    ) &
                    eval "jr_state_$jr_n=unpack"
                    jr_unpack=$((jr_unpack + 1))
                    jr_done=$((jr_done + jr_size)) ;;
                unpack)
                    jr_done=$((jr_done + jr_size))
                    case "$(cat "$root/tmp/rust-$jr_n.unpack")" in
                        running) jr_unpack=$((jr_unpack + 1)) ;;
                        0) eval "jr_state_$jr_n=done"; jr_done=$((jr_done + jr_size)) ;;
                        *) step_fail "$jr_label" "Unpacking $jr_component failed"; return 1 ;;
                    esac ;;
                done) jr_done=$((jr_done + 2 * jr_size)) ;;
            esac
            jr_total=$((jr_total + 2 * jr_size))
            eval "jr_state=\$jr_state_$jr_n"
            [ "$jr_state" = done ] || jr_busy=1
        done
        if [ "$jr_down" -gt 0 ]; then jr_doing=downloading
        elif [ "$jr_unpack" -gt 0 ]; then jr_doing=unpacking
        else jr_doing=checking; fi
        step "$jr_label" "now|$jr_done|$jr_total|MB|$((jr_total / 2))|$jr_doing"
        [ "$jr_busy" = 1 ] || break
        sleep 0.2
    done
    wait
    prepare_host_tools "$jr_staging"
    case "$("$jr_staging/bin/rustc" --version 2>/dev/null)" in
        "rustc $jr_version "*) ;;
        *) step_fail "$jr_label" 'The private Rust did not pass its version check'; return 1 ;;
    esac
    "$jr_staging/bin/cargo" --version >/dev/null 2>&1 || { step_fail "$jr_label" 'The private cargo does not run'; return 1; }
    printf '%s' "$jr_version $triple" > "$jr_staging/.toolchain-version"
    if [ -e "$private_rust" ]; then remove_inside "$private_rust"; fi
    mv "$jr_staging" "$private_rust"
    log "Rust $jr_version installed: $((jr_total / 2 / 1048576)) MB in $(($(now) - jr_started)) s"
    step "$jr_label" "done|$((jr_total / 2 / 1048576)) MB · $(($(now) - jr_started)) s"
}

# ----------------------------------------------------------- source job ---
# One repository of a release: its pack is downloaded (public ones in byte
# ranges when the host offers them), checked against the catalog's size and
# SHA-256, and checked out in a staging folder that is renamed into place;
# a repository nested inside another starts writing once its parent is
# there. Unpacking one pack overlaps the downloads of the others.
job_pack() { # job_pack LABEL DEST NAME PATH COMMIT SHA BYTES APP RELEASE PUBLIC HAVE PARENT
    jp_label=$1 jp_dest=$2 jp_name=$3 jp_path=$4 jp_commit=$5 jp_sha=$6 jp_bytes=$7 jp_app=$8 jp_release=$9
    shift 9
    jp_public=$1 jp_have=$2 jp_parent=$3
    jp_started=$(now)
    jp_pack="$root/cache/$jp_sha.pack"
    mkdir -p "$root/cache" "$jp_dest/.builder-repositories"
    if [ -f "$jp_pack" ] && [ "$(size_of "$jp_pack")" = "$jp_bytes" ] && [ "$(sha256 "$jp_pack")" = "$jp_sha" ]; then
        :
    else
        if [ "$jp_public" = true ]; then
            jp_url="$service/public/source/$jp_release/makepad.pack"
            jp_ranges=no
            [ "$(head_size "$jp_url")" = "$jp_bytes" ] && jp_ranges=yes
            fetch "$jp_url" "$jp_pack" "$jp_bytes" "$jp_ranges"
        else
            # Licensed sources: one connection (each request counts toward the
            # download limit); an update names the pack it replaces.
            jp_url="$service/source/$jp_app/$jp_release/$jp_name.pack"
            if [ -n "$jp_have" ]; then
                fetch "$jp_url" "$jp_pack" "$jp_bytes" no -X POST -H 'Content-Length: 0' -H @"$email_header_file" -H "X-Makepad-Have: $jp_have"
            else
                fetch "$jp_url" "$jp_pack" "$jp_bytes" no -X POST -H 'Content-Length: 0' -H @"$email_header_file"
            fi
        fi
        jp_parts=$fetch_parts
        while :; do
            jp_status=$(cat "$jp_pack.status" 2>/dev/null || echo running)
            fetch_bytes "$jp_pack" "$jp_parts"
            step "$jp_label" "now|$fetch_bytes|$((2 * jp_bytes))|MB|$jp_bytes|downloading"
            [ "$jp_status" = running ] || break
            sleep 0.2
        done
        if [ "$jp_status" != 0 ]; then
            if [ "$(cat "$jp_pack.code" 2>/dev/null || echo 200)" != 200 ] && [ -s "$jp_pack.part0" ] && [ "$(size_of "$jp_pack.part0")" -lt 2048 ]; then
                jp_reason=$(http_error "$jp_pack.part0"); : > "$jp_pack.part0"
                step_fail "$jp_label" "${jp_reason:-Download failed}"
            else
                step_fail "$jp_label" 'Download failed (see builder.log)'
            fi
            return 1
        fi
        fetch_join "$jp_pack" "$jp_parts" "$jp_bytes" || { step_fail "$jp_label" 'Download failed size verification'; return 1; }
        step "$jp_label" "now|$jp_bytes|$((2 * jp_bytes))|MB|$jp_bytes|checking"
        if [ "$(size_of "$jp_pack")" != "$jp_bytes" ] || [ "$(sha256 "$jp_pack")" != "$jp_sha" ]; then
            rm -f "$jp_pack"
            step_fail "$jp_label" "$jp_name download failed size/hash verification"
            return 1
        fi
    fi
    # A nested repository waits for its parent's checkout.
    if [ -n "$jp_parent" ]; then
        while [ ! -f "$jp_dest/.builder-repositories/$jp_parent" ]; do
            step_key "$jp_parent source"
            case "$(cut -d'|' -f1 "$steps_dir/$step_key" 2>/dev/null || :)" in
                failed | held) step "$jp_label" "held|stopped because $jp_parent failed"; return 1 ;;
            esac
            step "$jp_label" "now|$jp_bytes|$((2 * jp_bytes))|MB|$jp_bytes|waiting for $jp_parent"
            sleep 0.2
        done
    fi
    jp_checkout="$jp_dest/$jp_path"
    if [ -e "$jp_checkout" ]; then
        step_fail "$jp_label" "Existing source at $jp_checkout has no matching receipt; it was left unchanged"
        return 1
    fi
    jp_stage="$jp_dest/.$jp_name-staging"
    [ ! -e "$jp_stage" ] || remove_inside "$jp_stage"
    step "$jp_label" "now|$jp_bytes|$((2 * jp_bytes))|MB|$jp_bytes|unpacking"
    git init -q "$jp_stage" 2>>"$log_file"
    [ "$jp_name" != makepad ] || git -C "$jp_stage" config remote.origin.url https://github.com/makepad/makepad.git
    git -C "$jp_stage" index-pack --stdin < "$jp_pack" >/dev/null 2>>"$log_file" || { step_fail "$jp_label" 'The pack could not be unpacked'; return 1; }
    printf '%s\n' "$jp_commit" > "$jp_stage/.git/shallow"
    step "$jp_label" "now|$((jp_bytes * 3 / 2))|$((2 * jp_bytes))|MB|$jp_bytes|writing files"
    git -C "$jp_stage" -c advice.detachedHead=false checkout -q -f --detach "$jp_commit" 2>>"$log_file" || { step_fail "$jp_label" 'The source could not be checked out'; return 1; }
    [ "$(git -C "$jp_stage" rev-parse HEAD)" = "$jp_commit" ] || { step_fail "$jp_label" 'The source is not the expected commit'; return 1; }
    mkdir -p "$(dirname "$jp_checkout")"
    mv "$jp_stage" "$jp_checkout"
    put "$jp_dest/.builder-repositories/$jp_name" "$jp_commit $jp_sha"
    log "$jp_name $jp_commit checked out in $jp_dest"
    step "$jp_label" "done|$((jp_bytes / 1048576)) MB · $(($(now) - jp_started)) s"
}

# ---------------------------------------------------------------- jobs ---
# jobs_start KEY CMD...: CMD in the background; jobs_wait redraws until all
# are finished and fails when one failed.
running_jobs=
job_start() {
    ( "$@" ) </dev/null >/dev/null 2>>"$log_file" &
    running_jobs="$running_jobs $!"
}
jobs_wait() {
    jw_status=0
    # Escape stops waiting for Apple's installer (the other jobs go on);
    # only then are keys read here.
    while :; do
        jw_alive=
        for jw_pid in $running_jobs; do
            if kill -0 "$jw_pid" 2>/dev/null; then jw_alive="$jw_alive $jw_pid"; fi
        done
        [ -z "${tui:-}" ] || work_redraw
        [ -n "$jw_alive" ] || break
        if [ -n "${tui:-}" ] && [ -n "${xcode_job:-}" ] && kill -0 "$xcode_job" 2>/dev/null; then
            if key_read 1 1 && [ "$kr" = "$e" ]; then
                kill_tree "$xcode_job"
                step 'Xcode tools' 'held|stopped waiting; select Xcode tools to check again'
            fi
        else
            sleep 0.1
        fi
    done
    for jw_pid in $running_jobs; do wait "$jw_pid" || jw_status=1; done
    running_jobs= xcode_job=
    # A job that ended without saying so failed unexpectedly.
    for jw_key in $work_labels_keys; do
        case "$(cut -d'|' -f1 "$steps_dir/$jw_key" 2>/dev/null || :)" in
            now) put "$steps_dir/$jw_key" 'failed|stopped unexpectedly (see builder.log)'; jw_status=1 ;;
            failed | held) jw_status=1 ;;
        esac
    done
    [ -z "${tui:-}" ] || work_redraw
    return "$jw_status"
}

# ---------------------------------------------------------- source sets ---
# checkout_jobs: one job per repository of the loaded release that is not
# here yet, labelled "<Name> source". Sets checkout_labels.
checkout_jobs() {
    rel_dir
    checkout_labels=
    cj_dest=$rel_directory
    while IFS='|' read -r v_name v_path v_commit v_sha v_bytes; do
        repo_installed "$cj_dest" "$v_name" "$v_path" "$v_commit" && continue
        cj_parent=$(printf '%s\n' "$rel_repos" | awk -F'|' -v p="$v_path" '$2 != p && index(p "/", $2 "/") == 1 { n = $1 } END { print n }')
        cj_have=
        if [ "$rel_public" != true ] && [ -f "$root/installed/$rel_id.json" ]; then
            cj_have=$(json flat < "$root/installed/$rel_id.json" | awk -F'\t' -v name="$v_name" '$1 ~ /^repositories\/[0-9]+\/name$/ && $2 == name { split($1, a, "/"); want = a[2] } $1 ~ /\/sha256$/ { split($1, a, "/"); sha[a[2]] = $2 } END { if (want != "") print sha[want] }')
            [ "$cj_have" != "$v_sha" ] || cj_have=
        fi
        cj_title=$(printf '%s' "$v_name" | awk '{ print toupper(substr($0, 1, 1)) substr($0, 2) }')
        cj_label="$cj_title source"
        checkout_labels="$checkout_labels$cj_label
"
        job_start job_pack "$cj_label" "$cj_dest" "$v_name" "$v_path" "$v_commit" "$v_sha" "$v_bytes" "$rel_id" "$rel_release" "$rel_public" "$cj_have" "$cj_parent"
    done <<EOF
$rel_repos
EOF
    return 0
}
source_labels() { # the labels checkout_jobs will use, without starting anything
    rel_dir
    source_labels=
    while IFS='|' read -r v_name v_path v_commit v_sha v_bytes; do
        repo_installed "$rel_directory" "$v_name" "$v_path" "$v_commit" && continue
        sl_title=$(printf '%s' "$v_name" | awk '{ print toupper(substr($0, 1, 1)) substr($0, 2) }')
        source_labels="$source_labels$sl_title source
"
    done <<EOF
$rel_repos
EOF
}

# save_changes APP PREVIOUS-RELEASE-FILE: before an update moves an app off
# the snapshot it was built from, the edits made there are saved as
# changes/<app>-<date>.diff (unified, new files included) and .files (one
# "M|A|D path" line each), and changes/<app>.merge names the diff. Updates
# never merge; the person's coding agent reapplies the diff.
save_changes() {
    rel_load "$2" || return 0
    rel_dir
    sv_dir=$rel_directory
    sv_diff="$root/tmp/changes.diff" sv_files="$root/tmp/changes.files"
    : > "$sv_diff"; : > "$sv_files"
    while IFS='|' read -r v_name v_path v_commit v_sha v_bytes; do
        sv_co="$sv_dir/$v_path"
        [ -d "$sv_co/.git" ] || continue
        # Nested repositories report their own edits.
        sv_exclude=$(printf '%s\n' "$rel_repos" | awk -F'|' -v p="$v_path" 'index($2, p "/") == 1 { print ":(exclude)" substr($2, length(p) + 2) }')
        # shellcheck disable=SC2086 # one pathspec per line, none with spaces
        git -C "$sv_co" diff --name-status HEAD -- . $sv_exclude 2>/dev/null | while IFS='	' read -r sv_kind sv_path; do
            case "$sv_kind" in D*) sv_kind=D ;; A*) sv_kind=A ;; *) sv_kind=M ;; esac
            printf '%s|%s/%s\n' "$sv_kind" "$v_path" "$sv_path"
        done >> "$sv_files"
        # shellcheck disable=SC2086
        git -C "$sv_co" diff --binary HEAD --src-prefix="a/$v_path/" --dst-prefix="b/$v_path/" -- . $sv_exclude >> "$sv_diff" 2>/dev/null || :
        # shellcheck disable=SC2086
        git -C "$sv_co" ls-files --others --exclude-standard -- . $sv_exclude 2>/dev/null | while IFS= read -r sv_path; do
            case "$sv_path" in target/* | */target/*) continue ;; esac
            printf 'A|%s/%s\n' "$v_path" "$sv_path" >> "$sv_files"
            git -C "$sv_co" diff --no-index --binary --src-prefix="a/$v_path/" --dst-prefix="b/$v_path/" /dev/null "$sv_path" >> "$sv_diff" 2>/dev/null || :
        done
    done <<EOF
$rel_repos
EOF
    [ -s "$sv_files" ] || return 0
    mkdir -p "$root/changes"
    sv_name="$1-$(date +%Y-%m-%d)" sv_n=2
    while [ -e "$root/changes/$sv_name.diff" ]; do sv_name="$1-$(date +%Y-%m-%d)-$sv_n"; sv_n=$((sv_n + 1)); done
    mv "$sv_diff" "$root/changes/$sv_name.diff"
    mv "$sv_files" "$root/changes/$sv_name.files"
    put "$root/changes/$1.merge" "$sv_name.diff"
    log "Saved your changes to $1 in changes/$sv_name.diff before updating; the edited source stays in $sv_dir"
}
keep_edits() { # keep_edits APP NEW-RELEASE-FILE
    [ -f "$root/installed/$1.json" ] || return 0
    [ ! -f "$root/changes/$1.merge" ] || return 0
    rel_load "$2" || return 0
    rel_dir; ke_new_dir=$rel_directory ke_new=$rel_release
    rel_load "$root/installed/$1.json" || return 0
    rel_dir
    [ "$rel_release" != "$ke_new" ] && [ "$rel_directory" != "$ke_new_dir" ] || return 0
    # Saved once per update: after the merge the diff is not taken again.
    [ "$(cat "$root/changes/$1.saved" 2>/dev/null || :)" != "$rel_release $ke_new" ] || return 0
    ke_old=$rel_release
    save_changes "$1" "$root/installed/$1.json"
    mkdir -p "$root/changes"
    put "$root/changes/$1.saved" "$ke_old $ke_new"
}

# ------------------------------------------------------------- compiling ---
# How many crates this build produces, for the bar: the count of its last
# successful build, else Cargo's unit graph (no compiling). 0 when unknown.
expected_crates() {
    ec_record="$rel_target_dir/$rel_binary.crates"
    if [ -s "$ec_record" ]; then cat "$ec_record"; return; fi
    # The same features the build passes (build_features); an unset one
    # stopped this under set -u, and the bar never got its end.
    build_features
    # shellcheck disable=SC2086 # feature_args is fixed words
    (cd "$rel_directory/$rel_workspace" && RUSTC_BOOTSTRAP=1 "$CARGO" build --release -p "$rel_package" --bin "$rel_binary" $feature_args --unit-graph -Z unstable-options 2>/dev/null) |
        grep -o '"mode":"build"' | wc -l | tr -d ' '
}
# build_features -> $feature_args: the release's features, as cargo takes them.
build_features() {
    feature_args=--no-default-features
    [ -z "$rel_features" ] || feature_args="$feature_args --features $rel_features"
}
# compile_app: builds the loaded release's app in the background, with its
# progress on the step "Compile"; build log in the snapshot's target.
job_compile() { # job_compile LABEL APP
    jc_label=$1
    jc_started=$(now)
    step "$jc_label" "now|0|0|none|0|resolving"
    ( cargo_env "$rel_label" "$2" 1 >/dev/null; expected_crates > "$root/tmp/expected-crates" ) || :
    jc_expected=$(cat "$root/tmp/expected-crates" 2>/dev/null || echo 0)
    build_app "$2" || { step_fail "$jc_label" "$build_error"; return 1; }
    publish_app || { step_fail "$jc_label" "$publish_error"; return 1; }
    step "$jc_label" "done|$build_crates crates · $(($(now) - jc_started)) s"
}
# build_app APP: cargo build of the loaded release into the snapshot's
# target (log: builder-build.log; the TUI shows the crate count on the step
# "$jc_label"). With the parallel frontend, a crash (an internal compiler
# error, the deadlock detector, a compiler killed by a signal, no memory) or
# 10 minutes without output is retried once with one thread; the parallel
# log is kept as builder-build.parallel.log, and when one thread works,
# rustc-threads says 1 from then on. Ordinary compile errors are not retried.
build_app() {
    build_error= build_crates=0
    rustc_threads
    ba_threads=$rustc_threads
    ba_log="$rel_target_dir/builder-build.log"
    while :; do
        run_cargo_build "$1" "$ba_threads" && break
        ba_retry=0
        if [ "$ba_threads" -gt 1 ]; then
            if [ "$ba_stalled" = 1 ] || grep -Eq "internal compiler error|the compiler unexpectedly panicked|deadlock detected|rustc interrupted by|memory allocation of .* failed|didn.t exit successfully: .*--crate-name.*\(signal: " "$ba_log"; then ba_retry=1; fi
        fi
        if [ "$ba_retry" = 0 ]; then
            build_error=$(grep -m 1 '^error' "$ba_log" 2>/dev/null || :)
            cat "$ba_log" >> "$log_file"
            build_error="${build_error:-Build failed} (full log in builder.log)"
            return 1
        fi
        if [ "$ba_stalled" = 1 ]; then ba_reason='made no progress for 10 minutes'; else ba_reason='crashed'; fi
        cp "$ba_log" "$rel_target_dir/builder-build.parallel.log"
        log "The parallel Rust compiler ($ba_threads threads) $ba_reason; compiling again with one thread."
        ba_parallel=$ba_threads ba_threads=1
    done
    if [ "$ba_threads" = 1 ] && [ "${ba_parallel:-1}" -gt 1 ]; then
        put "$root/rustc-threads" "1
Serial since the parallel Rust compiler ($ba_parallel threads) $ba_reason and the serial one then compiled the same build. Delete this file to try the parallel compiler again."
        log 'Compiled with one thread. Builds here stay on one thread from now on (see rustc-threads).'
    fi
    build_crates=$(grep -c '"reason":"compiler-artifact"' "$ba_log" 2>/dev/null) || build_crates=0
    printf '%s\n' "$build_crates" > "$rel_target_dir/$rel_binary.crates"
}
run_cargo_build() { # run_cargo_build APP THREADS -> status; ba_stalled
    ba_stalled=0
    (
        cargo_env "$rel_label" "$1" "$2" >/dev/null
        build_features
        cd "$rel_directory/$rel_workspace"
        # shellcheck disable=SC2086 # feature_args is fixed words
        exec "$CARGO" build --release --message-format=json-render-diagnostics -p "$rel_package" --bin "$rel_binary" $feature_args
    ) </dev/null > "$ba_log" 2>&1 &
    rb_pid=$!
    rb_size=-1 rb_quiet=0 rb_ticks=0
    while kill -0 "$rb_pid" 2>/dev/null; do
        if [ -n "${jc_label:-}" ]; then
            rb_crates=$(grep -c '"reason":"compiler-artifact"' "$ba_log" 2>/dev/null) || rb_crates=0
            rb_fresh=$(grep '"reason":"compiler-artifact"' "$ba_log" 2>/dev/null | grep -c '"fresh":true') || rb_fresh=0
            rb_built=$((rb_crates - rb_fresh)) rb_of=0
            if [ "${jc_expected:-0}" -gt 0 ]; then
                rb_of=$((jc_expected - rb_fresh))
                [ "$rb_of" -gt "$rb_built" ] || rb_of=$((rb_built + 1))
            fi
            step "$jc_label" "now|$rb_built|$rb_of|crates|0|compiling"
        fi
        sleep 0.3
        rb_ticks=$((rb_ticks + 1))
        [ $((rb_ticks % 3)) = 0 ] || continue
        rb_now=$(size_of "$ba_log")
        if [ "$rb_now" = "$rb_size" ]; then rb_quiet=$((rb_quiet + 1)); else rb_size=$rb_now rb_quiet=0; fi
        if [ "$2" -gt 1 ] && [ "$rb_quiet" -ge 600 ]; then
            kill_tree "$rb_pid"
            ba_stalled=1
        fi
    done
    wait "$rb_pid"
}
# Background jobs ignore Ctrl+C; stopping one stops its children too.
kill_tree() {
    for kt_child in $(pgrep -P "$1" 2>/dev/null); do kill_tree "$kt_child"; done
    kill -TERM "$1" 2>/dev/null || :
}

# publish_app: the built executable as builder/<binary>.bin with the map
# from crate names to their source folders beside it (fonts, icons and other
# resources stay in the sources), its <binary> command beside this script,
# and on macOS a real <Title>.app there for the Dock.
publish_app() {
    publish_error=
    pa_log="$rel_target_dir/builder-build.log"
    pa_map="$root/tmp/package-paths"
    : > "$pa_map"
    awk '
        index($0, "\"reason\":\"compiler-artifact\"") {
            if (!match($0, /"manifest_path":"[^"]*"/)) next
            m = substr($0, RSTART + 17, RLENGTH - 18)
            t = substr($0, index($0, "\"target\":{"))
            k = substr(t, index(t, "\"kind\":[") + 8); k = substr(k, 1, index(k, "]") - 1)
            if (k == "\"custom-build\"") next
            if (!match(t, /"name":"[^"]*"/)) next
            name = substr(t, RSTART + 8, RLENGTH - 9); gsub(/-/, "_", name)
            sub(/\/[^\/]*$/, "", m)
            print name "\t" m
        }' "$pa_log" | sort -u > "$root/tmp/artifacts"
    while IFS='	' read -r pa_name pa_dir; do
        case "$pa_dir" in "$rel_directory"/*) ;; *) [ -d "$pa_dir/resources" ] || continue ;; esac
        case "$pa_dir" in "$root"/*) ;; *) publish_error="Resources for $pa_name are outside the portable installation"; return 1 ;; esac
        printf '%s\t%s\n' "$pa_name" "${pa_dir#"$root"/}" >> "$pa_map"
    done < "$root/tmp/artifacts"
    [ -s "$pa_map" ] || { publish_error='Cargo produced no application resource paths'; return 1; }
    pa_bin="$root/$rel_binary.bin"
    mv -f "$pa_map" "$pa_bin.makepad-package-paths"
    # Older runtimes read one shared map: keep other apps' entries.
    { cat "$pa_bin.makepad-package-paths"; cat "$root/makepad-package-paths" 2>/dev/null || :; } |
        awk -F'\t' '!seen[$1]++' > "$root/makepad-package-paths.next"
    mv -f "$root/makepad-package-paths.next" "$root/makepad-package-paths"
    cp "$rel_target_dir/release/$rel_binary" "$pa_bin.next"
    chmod 755 "$pa_bin.next"
    mv -f "$pa_bin.next" "$pa_bin"
    # This snapshot's artifacts are in the shared target now: later releases
    # at the same commits build from here.
    put "$rel_directory/.builder-built" "$rel_id $rel_release"
    write_launcher "$rel_binary"
    if [ "$plat" = mac ]; then mac_bundle || return 1; fi
    mkdir -p "$root/installed"
    cp "$release_file" "$root/installed/$rel_id.json.next"
    mv -f "$root/installed/$rel_id.json.next" "$root/installed/$rel_id.json"
    cp "$root/installed/$rel_id.json" "$root/installed-release.json"
    cp "$root/installed/$rel_id.json" "$rel_directory/$rel_id.json"
}

# write_launcher BINARY: the <binary> command beside this script. It keeps
# the invoking directory as the app's project; on macOS it opens the app's
# bundle, so the shell command and the Dock icon are the same app.
write_launcher() {
    cat > "$home/$1.next" <<EOF
#!/bin/sh
# Opens $1, built by the Makepad Builder in this folder (./makepad build $1).
set -eu
builder_script=\$0
while [ -L "\$builder_script" ]; do
    builder_directory=\$(cd -P -- "\$(dirname -- "\$builder_script")" && pwd)
    builder_link=\$(readlink "\$builder_script")
    case "\$builder_link" in /*) builder_script=\$builder_link ;; *) builder_script=\$builder_directory/\$builder_link ;; esac
done
builder_directory=\$(cd -P -- "\$(dirname -- "\$builder_script")" && pwd)
unset MAKEPAD_LOADER_EMAIL MAKEPAD_PACKAGE_DIR
if [ "\$#" -gt 0 ]; then
    case "\$1" in -*) ;; *) set -- --cwd "\$@" ;; esac
fi
if [ "\$(uname -s)" = Darwin ]; then
    for builder_app in "\$builder_directory/"*.app; do
        if [ -x "\$builder_app/Contents/MacOS/$1" ]; then
            builder_has_cwd=0
            for builder_arg in "\$@"; do
                [ "\$builder_arg" != --cwd ] || builder_has_cwd=1
            done
            if [ "\$builder_has_cwd" = 0 ]; then set -- --cwd "\$PWD" "\$@"; fi
            exec "\$builder_app/Contents/MacOS/$1" "\$@"
        fi
    done
fi
exec "\$builder_directory/builder/$1.bin" "\$@"
EOF
    chmod 755 "$home/$1.next"
    mv -f "$home/$1.next" "$home/$1"
}
# macos_launcher BINARY: the executable a bundle's Dock launch runs. It
# resolves everything from the bundle, so the whole folder can move.
macos_launcher() {
    cat <<EOF
#!/bin/sh
set -eu
app_dir=\$(cd -P -- "\$(dirname -- "\$0")" && pwd)
install_dir=\$(cd -P -- "\$app_dir/../../../builder" && pwd)
unset MAKEPAD_LOADER_EMAIL MAKEPAD_PACKAGE_DIR
has_project=0
for arg in "\$@"; do
    [ "\$arg" != --cwd ] || has_project=1
done
if [ "\$has_project" = 0 ]; then
    project=\$(cat "\$install_dir/installed/$1.project")
    case "\$project" in /*) ;; *) project="\$install_dir/\$project" ;; esac
    set -- --cwd "\$project" "\$@"
fi
mkdir -p "\$install_dir/target"
exec "\$app_dir/$1-bin" "\$@" </dev/null >>"\$install_dir/target/$1-app.log" 2>&1
EOF
}
# mac_bundle: "<Title>.app" beside this script with the executable, a
# launcher, a relative link to the Builder's folder (resources stay in the
# sources) and the app's icon, so it can be kept in the Dock.
mac_bundle() {
    mb_name=$(printf '%s' "$rel_title" | tr -cd 'A-Za-z0-9 ._-' | sed 's/^[ .]*//; s/ *$//')
    [ -n "$mb_name" ] || mb_name=$(printf '%s' "$rel_binary" | awk '{ print toupper(substr($0, 1, 1)) substr($0, 2) }')
    mb_bundle="$home/$mb_name.app"
    mb_tools="$rel_directory/$rel_makepad_path/tools/makepad_builder"
    mb_project="$rel_directory/$rel_makepad_path"
    mkdir -p "$root/installed"
    put "$root/installed/$rel_binary.project" "${mb_project#"$root"/}"
    mb_launcher=$(macos_launcher "$rel_binary")
    mb_icon=
    if [ "$rel_id" = scope ] && [ -f "$mb_tools/resources/scope.icns" ]; then mb_icon="$mb_tools/resources/scope.icns"
    elif [ -f "$rel_directory/$rel_workspace/resources/icon.icns" ]; then mb_icon="$rel_directory/$rel_workspace/resources/icon.icns"
    elif [ -f "$rel_directory/$rel_workspace/resources/icon_1024.png" ]; then mb_icon="$rel_directory/$rel_workspace/resources/icon_1024.png"
    else mb_icon=$(declared_icon); fi
    mb_stamp="bundle-v1 $(cksum < "$root/$rel_binary.bin") $(cksum < "$root/$rel_binary.bin.makepad-package-paths") $rel_title $(printf '%s' "$mb_launcher" | cksum) $mb_icon"
    if [ "$(cat "$mb_bundle/Contents/Resources/build-receipt" 2>/dev/null || :)" = "$mb_stamp" ] && [ -x "$mb_bundle/Contents/MacOS/$rel_binary" ]; then
        return 0
    fi
    mb_stage="$home/.$mb_name.app.next"
    [ ! -e "$mb_stage" ] || remove_inside "$mb_stage"
    mkdir -p "$mb_stage/Contents/MacOS" "$mb_stage/Contents/Resources"
    cp "$root/$rel_binary.bin" "$mb_stage/Contents/MacOS/$rel_binary-bin"
    printf '%s\n' "$mb_launcher" > "$mb_stage/Contents/MacOS/$rel_binary"
    chmod 755 "$mb_stage/Contents/MacOS/$rel_binary"
    ln -s ../../../builder "$mb_stage/Contents/MacOS/installation"
    sed 's#	#	installation/#' "$root/$rel_binary.bin.makepad-package-paths" > "$mb_stage/Contents/MacOS/$rel_binary-bin.makepad-package-paths"
    mb_icon_key=
    case "$mb_icon" in
        *.png) cp "$mb_icon" "$mb_stage/Contents/Resources/AppIcon.png"; mb_icon_key='<key>CFBundleIconFile</key><string>AppIcon.png</string>' ;;
        ?*) cp "$mb_icon" "$mb_stage/Contents/Resources/AppIcon.icns"; mb_icon_key='<key>CFBundleIconFile</key><string>AppIcon.icns</string>' ;;
    esac
    mb_title=$(printf '%s' "$rel_title" | sed 's/&/\&amp;/g; s/</\&lt;/g; s/>/\&gt;/g')
    cat > "$mb_stage/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>$rel_binary</string>
<key>CFBundleIdentifier</key><string>nl.makepad.$(printf '%s' "$rel_id" | tr '_' '-')</string>
<key>CFBundleName</key><string>$mb_title</string>
<key>CFBundleDisplayName</key><string>$mb_title</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleVersion</key><string>1.0</string>
<key>CFBundleShortVersionString</key><string>1.0</string>
<key>LSUIElement</key><false/>
<key>NSHighResolutionCapable</key><true/>
<key>NSLocationUsageDescription</key><string>Used to show your position on the map.</string>
<key>NSMicrophoneUsageDescription</key><string>Used for audio input in Makepad apps.</string>
<key>NSCameraUsageDescription</key><string>Used for camera input in Makepad apps.</string>
$mb_icon_key
</dict></plist>
EOF
    printf 'APPL????' > "$mb_stage/Contents/PkgInfo"
    put "$mb_stage/Contents/Resources/build-receipt" "$mb_stamp"
    mb_previous="$home/.$mb_name.app.previous"
    [ ! -e "$mb_previous" ] || remove_inside "$mb_previous"
    [ ! -e "$mb_bundle" ] || mv "$mb_bundle" "$mb_previous"
    mv "$mb_stage" "$mb_bundle"
    [ ! -e "$mb_previous" ] || remove_inside "$mb_previous"
}
# The icon an app's package declares for desktop bundles (as cargo makepad
# reads it): icon = "..." under [package.metadata.makepad.desktop].
declared_icon() {
    find "$rel_directory/$rel_workspace" -maxdepth 4 -name Cargo.toml -not -path '*/target/*' 2>/dev/null | while IFS= read -r di_manifest; do
        di_icon=$(awk -v package="$rel_package" '
            /^\[/ { table = $0; next }
            table == "[package]" && $1 == "name" { v = $0; sub(/^[^"]*"/, "", v); sub(/".*/, "", v); name = v }
            table == "[package.metadata.makepad.desktop]" && $1 == "icon" { v = $0; sub(/^[^"]*"/, "", v); sub(/".*/, "", v); icon = v }
            END { if (name == package && icon != "") print icon }' "$di_manifest")
        if [ -n "$di_icon" ] && [ -f "$(dirname "$di_manifest")/$di_icon" ]; then
            printf '%s\n' "$(dirname "$di_manifest")/$di_icon"
            break
        fi
    done
}

# launch_app: the built app, detached from this terminal (closing it does
# not stop the app), with its output in the snapshot's target folder.
launch_app() { # after rel_load/rel_dir of the installed release
    rel_target
    la_project="$rel_directory/$rel_makepad_path"
    la_log="$rel_target_dir/$rel_binary-app.log"
    mkdir -p "$rel_target_dir"
    if [ "$plat" = mac ]; then
        la_name=$(printf '%s' "$rel_title" | tr -cd 'A-Za-z0-9 ._-' | sed 's/^[ .]*//; s/ *$//')
        [ -n "$la_name" ] || la_name=$(printf '%s' "$rel_binary" | awk '{ print toupper(substr($0, 1, 1)) substr($0, 2) }')
        set -- "$home/$la_name.app/Contents/MacOS/$rel_binary" --cwd "$la_project"
        [ "$rel_id" != scope ] || set -- "$@" --focus
    else
        set -- "$home/$rel_binary" --cwd "$la_project"
    fi
    (
        cargo_env "$rel_label" "$rel_id" >/dev/null 2>&1
        unset MAKEPAD_LOADER_EMAIL RUSTUP_TOOLCHAIN
        export MAKEPAD_FEEDBACK_EMAIL="$email"
        cd "$la_project"
        if command -v setsid >/dev/null 2>&1; then exec setsid "$@"
        elif command -v perl >/dev/null 2>&1; then exec perl -e 'setpgrp(0, 0); exec { $ARGV[0] } @ARGV or die "$!\n"' "$@"
        else exec nohup "$@"; fi
    ) </dev/null >"$la_log" 2>&1 &
    log "Running $rel_title"
}
launched_message() {
    if [ "$plat" = mac ]; then message="${ok}✓${r0} $1 is running. ${dim}Right-click its Dock icon › Options › Keep in Dock.${r0}"
    else message="${ok}✓${r0} $1 is running. ${dim}Run it again from here or from its command.${r0}"; fi
}

# Remove source snapshots nothing refers to any more and that hold no
# edits (git status is clean apart from nested checkouts), with their build
# output. A snapshot you or an agent worked in stays.
prune_snapshots() {
    ps_keep=
    for ps_file in "$root"/installed/*.json "$root"/available/*.json; do
        [ -f "$ps_file" ] || continue
        rel_load "$ps_file" 2>/dev/null || continue
        rel_dir
        ps_keep="$ps_keep|$rel_directory|"
    done
    for ps_snap in "$root"/sources/*; do
        [ -d "$ps_snap" ] || continue
        case "$ps_keep" in *"|$ps_snap|"*) continue ;; esac
        ps_clean=1
        for ps_receipt in "$ps_snap"/.builder-repositories/*; do
            [ -f "$ps_receipt" ] || continue
            ps_name=${ps_receipt##*/}
            ps_path=$ps_name
            [ "$ps_name" != makepad ] || ps_path=makepad
            for ps_json in "$ps_snap"/*.json; do
                [ -f "$ps_json" ] || continue
                ps_found=$(json flat < "$ps_json" | awk -F'\t' -v n="$ps_name" '$1 ~ /\/name$/ && $2 == n { split($1, a, "/"); want = a[2] } $1 ~ /\/path$/ { split($1, a, "/"); path[a[2]] = $2 } END { if (want != "") print path[want] }')
                [ -z "$ps_found" ] || ps_path=$ps_found
            done
            [ -d "$ps_snap/$ps_path/.git" ] || { ps_clean=0; break; }
            ps_dirty=$(git -C "$ps_snap/$ps_path" status --porcelain 2>/dev/null | grep -v '^?? apps/[A-Za-z0-9_-]*/$' | head -n 1) || ps_dirty=error
            [ -z "$ps_dirty" ] || { ps_clean=0; break; }
        done
        [ "$ps_clean" = 1 ] || continue
        remove_inside "$ps_snap"
        [ ! -e "$root/target/sources/${ps_snap##*/}" ] || remove_inside "$root/target/sources/${ps_snap##*/}"
        log "Removed the unused sources ${ps_snap##*/}; nothing was built from them any more and they held no edits."
    done
}

# -------------------------------------------------------- coding agents ---
agent_directive='Before editing read the installation instructions in AGENTS.md in this folder (also named by the MAKEPAD_AGENT_CONTEXT environment variable). Rebuild with ../makepad build <app>. Preserve the existing repository instructions and wait for the user to choose a change.'
print_agent_context() {
    pac_rust=${rust_version:-pinned}
    cat <<EOF
# Makepad apps in this folder

Everything here is Makepad applications as Rust source (the folder above
holds only the \`makepad\` command and the built apps). The person using this
folder wants you to change these apps and rebuild them.

## Build and run

Always use the \`makepad\` command in the folder above (../makepad); do not
call cargo directly.
It uses this folder's compiler (a private Rust $pac_rust in toolchain/, or the
installed Rust recorded in selected-rust), keeps caches and build output in
this folder, and rebuilds the macOS app bundle so Dock icons keep working.

    ../makepad build <app>     compile after you change its source
    ../makepad run <app>       compile if needed, then open it
    ../makepad list            every app, its state and its source folder

A build prints compiler errors in full; fix them and build again. If you need
cargo itself (tests, clippy), first run: eval "\$(../makepad env)"
Do not install another compiler, change the global PATH or run rustup.

## Apps

| app | name | source |
|-----|------|--------|
EOF
    list_apps | while IFS='|' read -r pa_id pa_title pa_state pa_source; do
        printf '| %s | %s | %s |\n' "$pa_id" "$pa_title" "$pa_source"
    done
    cat <<EOF

Makepad itself (widgets, platform, shaders) is in the makepad folder of each
source snapshot; read its AGENTS.md before changing it. Each repository keeps
its own instructions. Fonts, icons and other resources stay in the sources;
each built app finds them through <app>.bin.makepad-package-paths.

## After an update

Updates never merge. When an app you changed gets a new release, the old
changes are saved first and the app is replaced by the clean new source:

    changes/<app>-<date>.diff    unified diff against the release you edited
    changes/<app>-<date>.files   one "M|A|D path" line per changed file

When asked to merge, reapply that diff onto the new source by editing files,
keep the intent of each change where the new release moved code, then
../makepad build <app> until it compiles. Report any change that no longer
applies.

## Rules

- Keep edits inside sources/. Built apps (<app>.bin, <Title>.app) are regenerated.
- Your licenses do not allow publishing the commercial sources.
- Do not read or expose makepad-builder.json or any email address.
EOF
}
# list_apps: id|title|state|source folder, licensed apps first.
list_apps() {
    la_ids=$(cat "$root/tmp/builder/licensed" 2>/dev/null || :)
    if [ -z "$la_ids" ]; then
        for la_file in "$root"/available/*.json; do
            [ -f "$la_file" ] || continue
            rel_load "$la_file" 2>/dev/null || continue
            [ "$rel_public" = true ] || la_ids="$la_ids $rel_id"
        done
    fi
    for la_id in $la_ids; do
        app_state "$la_id"
        rel_load "$root/available/$la_id.json" 2>/dev/null || continue
        rel_dir
        printf '%s|%s|%s|%s\n' "$la_id" "$rel_title" "$state" "${rel_directory#"$root"/}/$rel_workspace"
    done
    free_apps | while IFS='|' read -r la_id la_title; do
        app_state "$la_id"
        la_src=sources/makepad/apps/$la_id
        if app_release_file "$la_id" 2>/dev/null && rel_load "$release_file" 2>/dev/null; then rel_dir; la_src="${rel_directory#"$root"/}/$rel_workspace"; fi
        printf '%s|%s|%s|%s\n' "$la_id" "$la_title" "$state" "$la_src"
    done
}
write_agent_files() {
    print_agent_context > "$root/AGENTS.md.next" && mv -f "$root/AGENTS.md.next" "$root/AGENTS.md"
    cp "$root/AGENTS.md" "$root/CLAUDE.md"
    cp "$root/AGENTS.md" "$root/agent-context.txt"
}
# The newest installed release, for the environment agents and shells get.
agent_release() {
    for ar_file in "$root"/installed/*.json; do
        [ -f "$ar_file" ] || continue
        rel_load "$ar_file" 2>/dev/null || continue
        rel_dir
        rel_installed && return 0
    done
    return 1
}

# ---------------------------------------------------------------- session ---
# The folder's own records: the email (makepad-builder.json, never sent
# anywhere but the license service), a scratch folder, the activity log.
read_email() {
    email=
    for re_file in "$root/makepad-builder.json" "$root/makepad-loader.json"; do
        [ -f "$re_file" ] || continue
        email=$(json flat < "$re_file" | awk -F'\t' '$1 == "email" { print $2; exit }')
        break
    done
}
save_email() {
    put "$root/makepad-builder.json" "{\"version\":1,\"email\":\"$email\",\"app\":\"makepad\"}"
}
session() {
    mkdir -p "$root/tmp/builder/steps" "$root/tmp/builder/slots"
    scratch="$root/tmp/builder"
    steps_dir="$scratch/steps"
    slots="$scratch/slots"
    log_file="$root/builder.log"
    # Connection slots and step records of a stopped run are stale.
    for s_old in "$slots"/* "$steps_dir"/*; do [ ! -e "$s_old" ] || remove_inside "$s_old"; done
    read_email
    load_registry
}

# ----------------------------------------------------------- commands ---
# Plain, line-based output for coding agents: every compiler message, then
# one result line they can check.
die() { printf '%s\n' "$*" >&2; exit 1; }
cmd_setup() { # the app's release, its Rust and system tools, for build/run
    session
    app_release_file "$1" || die "Unknown app $1; ./makepad list shows every app."
    rel_load "$release_file" || die "$1 has no usable release; open ./makepad and update."
    rel_supported || die "$rel_title is not available for this platform yet."
    rust_ready "$rel_rust" || die "Rust $rel_rust is not set up here; open ./makepad to set it up."
    tools_check || die "The system developer tools are not ready ($tools_problem); open ./makepad to set them up."
}
cmd_build() { # cmd_build APP [run]
    cmd_setup "$1"
    rel_dir
    if ! rel_installed; then
        [ "$rel_public" = true ] || [ -n "$email" ] || die "Log in with the email that has the $rel_title license: open ./makepad."
        [ "$rel_public" = true ] || email_headers
        printf 'Downloading the %s source\n' "$rel_title"
        keep_edits "$1" "$release_file"
        rel_load "$release_file"
        work_labels_keys=
        source_labels
        while IFS= read -r cb_label; do
            [ -n "$cb_label" ] || continue
            step_key "$cb_label"; work_labels_keys="$work_labels_keys $step_key"
        done <<EOF
$source_labels
EOF
        checkout_jobs
        if ! jobs_wait; then
            for cb_key in $work_labels_keys; do cut -d'|' -f2 "$steps_dir/$cb_key" >&2; done
            die 'Download failed.'
        fi
        rel_load "$release_file"
        rel_dir
    fi
    prepare_target
    printf 'Compiling %s with Rust %s\n' "$rel_title" "$rel_rust"
    jc_label=
    if ! build_app "$rel_id"; then
        # Every compiler message for the agent, then the result.
        grep -v '^{' "$rel_target_dir/builder-build.log" >&2 || :
        die "Build failed: $rel_title"
    fi
    grep -v '^{' "$rel_target_dir/builder-build.log" | grep -v '^ *Compiling ' || :
    publish_app || die "$publish_error"
    printf 'Built %s: %s\n' "$rel_title" "$root/$rel_binary.bin"
    if [ "$plat" = mac ]; then printf 'Bundled %s\n' "$mb_bundle"; fi
    if [ "${2:-}" = run ]; then launch_app; printf 'Opened %s\n' "$rel_title"; fi
}
print_env() {
    session
    rust_version
    rust_ready "${rust_version:-0.0.0}" || die 'Rust is not set up here; open ./makepad to set it up.'
    rel_label=
    if agent_release; then :; elif rel_load "$root/available/makepad.json" 2>/dev/null; then rel_dir; fi
    [ -n "$rel_label" ] || die 'Download an app first; ./makepad list shows them.'
    (
        cargo_env "$rel_label" "${rel_id:-makepad}" >/dev/null
        for pe_var in CARGO_HOME CARGO_TARGET_DIR RUSTC CARGO RUSTDOC RUSTC_BOOTSTRAP TMPDIR TMP TEMP RUSTUP_HOME RUSTUP_AUTO_INSTALL RUSTUP_TOOLCHAIN MAKEPAD_LOADER_EMAIL MAKEPAD_PACKAGE_DIR CUDA_PATH CUDA_HOME CUDACXX MAKEPAD_GGML_NO_CUDA MAKEPAD_GGML_REQUIRE_CUDA RUSTFLAGS PATH; do
            eval "pe_value=\${$pe_var-}"
            printf "export %s='%s'\n" "$pe_var" "$(printf '%s' "$pe_value" | sed "s/'/'\\\\''/g")"
        done
    )
}

# migrate_layout: an earlier Builder kept everything in the folder itself.
# Its records, sources, toolchains and built binaries move into builder/
# (renames within one folder: nothing is copied or downloaded again), the
# app commands and bundles are rewritten to look there, and the folder
# keeps only this command and the apps.
migrate_layout() {
    [ -d "$home/installed" ] || [ -d "$home/toolchain" ] || [ -d "$home/sources" ] || [ -f "$home/makepad-builder.json" ] || [ -f "$home/makepad-loader.json" ] || return 0
    mkdir -p "$root"
    for ml_name in installed available sources target cache toolchain cargo-home rustup-home tmp changes selected-rust \
        latest.json installed-release.json agreements-accepted .login-asked graphics-notice-read makepad-builder.json \
        makepad-loader.json builder.log rustc-threads makepad-package-paths AGENTS.md CLAUDE.md agent-context.txt \
        makepad-builder .builder-version bootstrap-builder.sh run-builder.sh check-tools.sh rust-tools.sh; do
        if { [ -e "$home/$ml_name" ] || [ -L "$home/$ml_name" ]; } && [ ! -e "$root/$ml_name" ]; then
            mv "$home/$ml_name" "$root/$ml_name"
        fi
    done
    for ml_bin in "$home"/*.bin; do
        [ -f "$ml_bin" ] || continue
        ml_name=${ml_bin##*/}; ml_name=${ml_name%.bin}
        identifier "$ml_name" || continue
        mv -f "$ml_bin" "$root/$ml_name.bin"
        [ ! -f "$ml_bin.makepad-package-paths" ] || mv -f "$ml_bin.makepad-package-paths" "$root/$ml_name.bin.makepad-package-paths"
        [ ! -f "$home/$ml_name" ] || write_launcher "$ml_name"
    done
    for ml_app in "$home"/*.app; do
        [ -L "$ml_app/Contents/MacOS/installation" ] || continue
        ln -sfn ../../../builder "$ml_app/Contents/MacOS/installation"
        for ml_file in "$ml_app"/Contents/MacOS/*; do
            ml_name=${ml_file##*/}
            case "$ml_name" in *-bin | *.makepad-package-paths | installation) continue ;; esac
            identifier "$ml_name" || continue
            macos_launcher "$ml_name" > "$ml_file.next" && chmod 755 "$ml_file.next" && mv -f "$ml_file.next" "$ml_file"
        done
        # Rebuilt from the moved binary on the next build.
        rm -f "$ml_app/Contents/Resources/build-receipt"
    done
}
[ -z "$home" ] || migrate_layout

case "${1:-}" in
    build | run)
        [ -n "$root" ] || die "Run this command from your Makepad folder: ./makepad $1 APP"
        [ -n "${2:-}" ] || die "Usage: ./makepad $1 APP (./makepad list shows every app)"
        cmd_build "$2" "$1"; exit 0 ;;
    list)
        [ -n "$root" ] || die 'Run this command from your Makepad folder.'
        session
        list_apps | awk -F'|' '{ printf "%-14s %-22s %-9s %s\n", $1, $2, $3, $4 }'
        exit 0 ;;
    env)
        [ -n "$root" ] || die 'Run this command from your Makepad folder.'
        print_env; exit 0 ;;
    agent-context)
        [ -n "$root" ] || die 'Run this command from your Makepad folder.'
        session; rust_version; print_agent_context; exit 0 ;;
    rust-check)
        # The read-only Rust probe the Rust Builder shares:
        #   rust-check --probe MINIMUM TRIPLE | --validate MINIMUM TRIPLE SYSROOT
        # prints the sysroot; exit 1 without output when no Rust is found,
        # exit 2 with the reason when one is found but cannot be used.
        triple=${4:-$triple}
        [ "${2:-}" = --probe ] || [ "${2:-}" = --validate ] || die 'Usage: makepad rust-check --probe MIN TRIPLE | --validate MIN TRIPLE SYSROOT'
        if rust_check "$2" "$3" "${5:-}"; then printf '%s\n' "$rust_sysroot"; exit 0; fi
        [ "$rust_reason" != 'No installed Rust was found on PATH.' ] || exit 1
        printf '%s\n' "$rust_reason"; exit 2 ;;
    tools-check)
        # Exit 0 when the system developer tools are ready; else 1 and the
        # problem (missing, license or unsupported).
        if tools_check; then exit 0; fi
        printf '%s\n' "$tools_problem"; exit 1 ;;
    tools-setup)
        # Line-based setup of the system tools, for the Rust Builder.
        if tools_check; then printf '  The system developer tools are ready.\n'; exit 0; fi
        if [ "$plat" = mac ]; then
            if [ "$tools_problem" = license ]; then
                printf '  Xcode is installed, but its license has not been accepted yet.\n  Read and accept it with: sudo xcodebuild -license\n'
            else
                printf "  Opening Apple's installer for the command line developer tools.\n  When it has finished, run the Builder again.\n"
                /usr/bin/xcode-select --install >/dev/null 2>&1 || :
            fi
            exit 1
        fi
        linux_packages || die 'Install C/C++, git, curl, CMake, pkg-config and the OpenSSL, X11, Xcursor, xkbcommon, ALSA, PulseAudio, Wayland, EGL/GLX and GBM development packages, then run the Builder again.'
        printf '  Makepad compiles from source and needs development packages from %s:\n\n    sudo %s\n\n  Press Return to run it, or Ctrl+C to stop. ' "$distro_name" "$packages_cmd"
        IFS= read -r ts_answer || exit 1
        # shellcheck disable=SC2086
        sudo $packages_cmd
        tools_check || die 'Some development packages are still missing; see the package manager output above.'
        exit 0 ;;
    -h | --help) sed -n '2,15p' "$self"; exit 0 ;;
    '') ;;
    *) die "Unknown command $1; ./makepad --help lists them." ;;
esac

# ------------------------------------------------------------ terminal UI ---
exec 3<>/dev/tty || die 'The Makepad Builder needs an interactive terminal.'
exec 1>&3
tui=1
e=$(printf '\033')
dim="${e}[2m" b="${e}[1m" ok="${e}[32m" acc="${e}[36m" warn="${e}[33m" red="${e}[31m" inv="${e}[7m" r0="${e}[0m"
tty_saved=$(stty -g <&3)
# The window is titled "Makepad Builder" while it runs; the terminal's own
# title comes back afterwards.
screen_on() { printf '%s[22;0t%s]0;Makepad Builder\007%s[?1049h%s[?25l' "$e" "$e" "$e" "$e"; stty -icanon -echo min 1 time 0 <&3 2>/dev/null; }
screen_off() { stty "$tty_saved" <&3 2>/dev/null || :; printf '%s[?25h%s[?1049l%s]0;\007%s[23;0t' "$e" "$e" "$e" "$e"; }
# Background jobs ignore Ctrl+C (the shell starts them that way), so every
# exit stops them and all their children; their partial downloads stay for
# the next run.
on_exit() {
    oe_status=$?
    for oe_pid in $running_jobs; do kill_tree "$oe_pid"; done
    screen_off
    if [ -n "${quiet_exit:-}" ]; then printf '%s\n' "$quiet_exit"
    elif [ "$oe_status" = 0 ]; then
        if [ -n "$home" ] && [ -x "$home/makepad" ]; then printf 'Makepad Builder closed. Start it again with %s/makepad\n' "$home"
        else printf 'Makepad Builder closed.\n'; fi
    elif [ "$oe_status" != 130 ]; then
        printf 'The Makepad Builder stopped unexpectedly; the details are in %s\n' "${log_file:-builder.log}" >&2
    fi
}
trap on_exit EXIT
trap 'exit 130' INT TERM HUP
screen_on

size() {
    set -- $(stty size <&3 2>/dev/null || echo 24 80)
    height=${1:-24} width=${2:-80}
    [ "$width" -le 80 ] || width=80
}
# Display width of plain text (UTF-8 continuation bytes do not count).
dwidth() { printf '%s' "$1" | LC_ALL=C tr -d '\200-\277' | wc -c | tr -d ' '; }

polling=0
key_cr=$(printf '\r') key_del=$(printf '\177') key_bs=$(printf '\010') key_nak=$(printf '\025')
# key: one key press as $key. Under bash (macOS's /bin/sh, and the Builder
# re-runs itself under /bin/bash where dash started it) the read builtin
# takes it, so no process runs while the Builder waits; only a system
# without bash at all reads one byte at a time with dd. Polling (a measurement still
# running) returns key=none after about a second.
key_read() { # key_read COUNT WAIT -> kr (WAIT: 0 blocks, else seconds)
    if [ -n "${BASH_VERSION:-}" ]; then
        kr=
        if [ "$2" = 0 ]; then IFS= read -rsn "$1" -d '' kr <&3; kr_status=$?
        else IFS= read -rsn "$1" -d '' -t "$2" kr <&3; kr_status=$?; fi
        [ "$kr_status" -le 128 ] || kr_status=142
        return "$kr_status"
    fi
    if [ "$2" = 0 ]; then stty min 1 time 0 <&3 2>/dev/null; else stty min 0 time $(($2 * 10)) <&3 2>/dev/null; fi
    # No bash on this system at all: a byte-exact read (head would take the
    # rest of an arrow key's bytes with it).
    kr=$(dd bs=1 count="$1" <&3 2>/dev/null; printf x)
    kr=${kr%x}
    stty min 1 time 0 <&3 2>/dev/null
    [ -n "$kr" ] || { [ "$2" = 0 ] && return 1; return 142; }
}
key() {
    kw=0; [ "$polling" != 1 ] || kw=1
    key_read 1 "$kw" || { [ "$?" = 142 ] && { key=none; return; }; exit 1; }
    k=$kr
    case "$k" in
        "$e")
            key_read 2 1 || kr=
            case "$kr" in '[A' | OA) key=up ;; '[B' | OB) key=down ;; '[C' | OC) key=right ;; '[D' | OD) key=left ;; '') key=esc ;; *) key=other ;; esac ;;
        '
' | "$key_cr") key=enter ;;
        "$key_del" | "$key_bs") key=backspace ;;
        "$key_nak") key=clear ;;
        '') key=enter ;;
        *) key=$k ;;
    esac
}

# ---------------------------------------------------------------- drawing ---
# A screen is a list of rows: "head|TITLE", "note|TEXT", "step|LABEL" or
# "item|ID|NAME|LICENSE|STATUS|ACTION"; ACTION "=" means the status already
# says what Return does, so only ⏎ is added.
message= choice= footer_override=
screen_meta() {
    back=1 crumb= sub=
    case "$screen" in
        main) back=0 sub='Makepad apps ship as source, so your own coding agent can customize them.' ;;
        free) crumb=' › Makepad experiments' sub='From our open source repository: experiments, not finished applications.' ;;
        terms) crumb=' › License agreements' sub='Return opens an agreement in your browser.' ;;
        consent) crumb=" › $consent_title" sub='Please read what you are agreeing to.' ;;
        work) crumb=" › $work_title" sub=$work_sub ;;
        signin) crumb=' › Log in' sub='Welcome to the Makepad Builder.' ;;
        folder) crumb=' › Folder' sub='Everything the Builder installs goes into one folder.' ;;
        xcode) crumb=' › Apple developer tools' sub='Needed to compile on macOS.' ;;
        gpu) crumb=' › Graphics' sub='Before you continue.' ;;
        packages) crumb=' › System packages' sub='Installed outside this folder, so please check them.' ;;
    esac
    default_footer='↑↓ move   ⏎ select   esc back   q quit'
    [ "$back" = 1 ] || default_footer='↑↓ move   ⏎ select   q quit'
    case "$screen" in consent | gpu | xcode | packages) default_footer='↑↓ move   ⏎ select   esc cancel' ;; esac
}
screen_rows() {
    case "$screen" in
        main) main_rows ;;
        free) free_rows ;;
        terms) terms_rows ;;
        consent) consent_rows ;;
        work) printf 'note|\n'; printf '%s' "$work_labels" | sed 's/^/step|/' ;;
        signin) signin_rows ;;
        folder) folder_rows ;;
        xcode) xcode_rows ;;
        gpu) gpu_rows ;;
        packages) packages_rows ;;
    esac
}
item_line() { # item_line NAME LICENSE STATUS ACTION SELECTED -> il
    pad "$1" 15
    if [ "$5" = 1 ]; then il="  ${acc}›${r0} ${b}${padded}${r0} "; else il="    ${padded} "; fi
    case "$2" in
        commercial) il="${il}${b}commercial license  ${r0}" ;;
        beta) il="${il}${warn}beta access         ${r0}" ;;
        free) il="${il}${dim}free, open source   ${r0}" ;;
    esac
    il="${il}$3"
    if [ "$5" = 1 ]; then
        if [ "$4" = = ]; then il="${il} ${acc}⏎${r0}"
        elif [ -n "$4" ]; then
            if [ -z "$3" ] && [ -z "$2" ]; then il="${il}${acc}$4 ⏎${r0}"; else il="${il}  ${acc}$4 ⏎${r0}"; fi
        fi
    fi
}
# The line of a work step, from its state file.
step_line() { # step_line LABEL -> sl
    step_key "$1"
    sl_state= sl_a= sl_b= sl_unit= sl_bytes= sl_doing=
    IFS='|' read -r sl_state sl_a sl_b sl_unit sl_bytes sl_doing < "$steps_dir/$step_key" 2>/dev/null || sl_state=next
    pad "$1 " 16
    case "$sl_state" in
        done) sl="    ${ok}✓${r0} ${padded}${dim}${sl_a}${r0}" ;;
        failed) sl="    ${red}✗${r0} ${padded}${red}${sl_a}${r0}" ;;
        held) sl="    ${warn}◌${r0} ${padded}${dim}${sl_a}${r0}" ;;
        now)
            sl="    ${acc}●${r0} ${b}${padded}${r0}"
            sl_detail=
            if [ "${sl_b:-0}" -gt 0 ]; then
                if [ "$sl_unit" = crates ]; then sl_width=30; else sl_width=16; fi
                sl_fill=$((sl_a * sl_width / sl_b)); [ "$sl_fill" -le "$sl_width" ] || sl_fill=$sl_width
                sl_on= sl_off= sl_i=0
                while [ "$sl_i" -lt "$sl_width" ]; do
                    if [ "$sl_i" -lt "$sl_fill" ]; then sl_on="${sl_on}━"; else sl_off="${sl_off}─"; fi
                    sl_i=$((sl_i + 1))
                done
                sl_pct=$((sl_a * 100 / sl_b)); [ "$sl_pct" -le 100 ] || sl_pct=100
                sl_p="   $sl_pct"; sl_p=${sl_p#"${sl_p%???}"}
                sl="${sl}${acc}${sl_on}${r0}${dim}${sl_off}${r0} ${sl_p}%"
                if [ "$sl_unit" = crates ]; then
                    sl_detail="$sl_a / $sl_b crates"
                elif [ "$sl_unit" = MB ] && [ "${sl_bytes:-0}" -gt 0 ]; then
                    sl_mb_total=$((sl_bytes / 1048576))
                    sl_mb_done=$((sl_a * sl_mb_total / sl_b))
                    [ "$sl_mb_total" = 0 ] || sl_detail="$sl_mb_done/$sl_mb_total MB"
                    # Throughput and time left while it downloads.
                    sl_t=$(now)
                    eval "sl_t0=\${rate_t_$step_key:-} sl_b0=\${rate_b_$step_key:-}"
                    if [ -z "$sl_t0" ] || [ "$sl_doing" != downloading ]; then
                        eval "rate_t_$step_key=\$sl_t rate_b_$step_key=\$sl_mb_done"
                    elif [ "$sl_t" -gt "$sl_t0" ]; then
                        sl_rate=$(( (sl_mb_done - sl_b0) / (sl_t - sl_t0) ))
                        if [ "$sl_rate" -gt 0 ]; then
                            sl_left=$(( (sl_mb_total - sl_mb_done) / sl_rate ))
                            sl_detail="$sl_detail · $sl_rate MB/s"
                            [ "$sl_left" -lt 1 ] || sl_detail="$sl_detail · $sl_left s left"
                        fi
                    fi
                fi
            else
                spin_frame=$((${spin_frame:-0} + 1))
                case $((spin_frame % 10)) in 0) sl_c=⠋ ;; 1) sl_c=⠙ ;; 2) sl_c=⠹ ;; 3) sl_c=⠸ ;; 4) sl_c=⠼ ;; 5) sl_c=⠴ ;; 6) sl_c=⠦ ;; 7) sl_c=⠧ ;; 8) sl_c=⠇ ;; *) sl_c=⠏ ;; esac
                sl="${sl}${acc}${sl_c}${r0}"
                # No total yet: the crates compiled so far still count up.
                [ "$sl_unit" != crates ] || [ "${sl_a:-0}" = 0 ] || sl_detail="$sl_a crates"
            fi
            [ -z "$sl_doing" ] || sl_detail="${sl_detail:+$sl_detail · }$sl_doing"
            [ -z "$sl_detail" ] || sl="${sl}  ${dim}${sl_detail}${r0}" ;;
        *) sl="    ${dim}○ ${padded}${r0}" ;;
    esac
}

draw() {
    size
    screen_meta
    rows_out=$(screen_rows)
    body= nbody=0 sel_line=-1 d_item=0 d_first=1
    while IFS='|' read -r d_kind d_1 d_2 d_3 d_4 d_5; do
        case "$d_kind" in
            head)
                if [ "$d_first" = 0 ] || [ "$back" = 0 ]; then body="$body
"; nbody=$((nbody + 1)); fi
                body="$body    ${dim}${d_1}${r0}
"; nbody=$((nbody + 1)) ;;
            note) body="$body    ${d_1}
"; nbody=$((nbody + 1)) ;;
            step) step_line "$d_1"; body="$body$sl
"; nbody=$((nbody + 1)) ;;
            item)
                if [ "$d_item" = 0 ] && [ "$back" = 1 ]; then body="$body
"; nbody=$((nbody + 1)); fi
                d_sel=0; [ "$d_item" != "$sel" ] || { d_sel=1; sel_line=$nbody; }
                item_line "$d_2" "$d_3" "$d_4" "$d_5" "$d_sel"
                body="$body$il
"; nbody=$((nbody + 1)); d_item=$((d_item + 1)) ;;
        esac
        d_first=0
    done <<EOF
$rows_out
EOF
    items=$d_item
    room=$((height - 8 - back))
    [ "$room" -ge 3 ] || room=3
    if [ "$nbody" -le "$room" ]; then top=0; else
        [ "$back" = 1 ] || room=$((room - 1))
        if [ "$sel_line" -ge 0 ]; then
            [ "$sel_line" -ge "$top" ] || top=$sel_line
            [ "$sel_line" -lt $((top + room)) ] || top=$((sel_line - room + 1))
        fi
        [ "$top" -le $((nbody - room)) ] || top=$((nbody - room))
    fi
    d_left="Makepad commercial apps$crumb"
    d_email=${email:-not logged in}
    d_pad=$((width - 2 - $(dwidth "$d_left") - $(dwidth "$d_email") - 2))
    [ "$d_pad" -ge 1 ] || d_pad=1
    d_gap=; d_i=0; while [ "$d_i" -lt "$d_pad" ]; do d_gap="$d_gap "; d_i=$((d_i + 1)); done
    out="${e}[H${e}[K
  ${b}Makepad${r0} commercial apps${crumb}${d_gap}${dim}${d_email}${r0}${e}[K
  ${dim}${sub}${r0}${e}[K
"
    d_i=0 d_shown=0
    while IFS= read -r d_line; do
        if [ "$d_i" -ge "$top" ] && [ "$d_shown" -lt "$room" ]; then
            out="$out$d_line${e}[K
"
            d_shown=$((d_shown + 1))
        fi
        d_i=$((d_i + 1))
    done <<EOF
$body
EOF
    if [ "$nbody" -gt "$room" ]; then
        d_below=$((nbody - top - d_shown))
        if [ "$d_below" -gt 0 ]; then out="$out    ${dim}↓ $d_below more${r0}${e}[K
"; else out="$out    ${dim}↑ $top above${r0}${e}[K
"; fi
    elif [ "$back" = 1 ]; then out="$out${e}[K
"; fi
    d_rule=; d_i=4; while [ "$d_i" -lt "$width" ]; do d_rule="${d_rule}─"; d_i=$((d_i + 1)); done
    out="$out${e}[K
  ${dim}${d_rule}${r0}${e}[K
  ${message}${e}[K
  ${choice}${e}[K
  ${dim}${footer_override:-$default_footer}${r0}${e}[K
${e}[J"
    printf '%s' "$out"
}
# The work page's steps, redrawn in place about ten times a second.
work_redraw() {
    [ "$screen" = work ] || return 0
    wr_row=5 wr_out=
    while IFS= read -r wr_label; do
        [ -n "$wr_label" ] || continue
        step_line "$wr_label"
        wr_out="$wr_out${e}[${wr_row};1H$sl${e}[K"
        wr_row=$((wr_row + 1))
    done <<EOF
$work_labels
EOF
    printf '%s' "$wr_out"
}
busy() { message="${acc}⠋${r0} $1"; draw; }

# choose QUESTION NOTE DEFAULT OPTION...: the question on the status line and
# the options under it; ←→ or a first letter picks, ⏎ accepts, esc backs out.
choose() {
    ch_q=$1 ch_note=$2 ch_pick=$3; shift 3
    ch_saved_footer=$footer_override
    while :; do
        choice=
        for ch_opt in "$@"; do
            if [ "$ch_opt" = "$ch_pick" ]; then choice="${choice}${inv} ${ch_opt} ${r0} "; else choice="${choice} ${ch_opt}  "; fi
        done
        [ -z "$ch_note" ] || choice="${choice} ${dim}${ch_note}${r0}"
        message=$ch_q footer_override='←→ choose   ⏎ accept   esc back'
        draw
        key
        case "$key" in
            enter) chosen=$ch_pick; break ;;
            esc | q) chosen=; break ;;
            left | up | right | down)
                ch_prev= ch_next= ch_found=
                for ch_opt in "$@"; do
                    if [ -n "$ch_found" ] && [ -z "$ch_next" ]; then ch_next=$ch_opt; fi
                    [ "$ch_opt" != "$ch_pick" ] || ch_found=1
                    [ -n "$ch_found" ] || ch_prev=$ch_opt
                done
                case "$key" in left | up) [ -z "$ch_prev" ] || ch_pick=$ch_prev ;; *) [ -z "$ch_next" ] || ch_pick=$ch_next ;; esac ;;
            *)
                ch_hit=
                for ch_opt in "$@"; do
                    case "$ch_opt" in "$key"*) ch_hit=$ch_opt; break ;; esac
                done
                if [ -n "$ch_hit" ]; then chosen=$ch_hit; break; fi ;;
        esac
    done
    message= choice= footer_override=$ch_saved_footer
    [ -n "$chosen" ]
}

# edit PROMPT INITIAL HINT FOOTER: a one-line editor on the status line.
# Escape returns 1. Sets edited.
edit() {
    edited=$2
    ed_saved_footer=$footer_override
    while :; do
        message="$1 ${b}${edited}${r0}${inv} ${r0}" choice=$3 footer_override=$4
        draw
        key
        case "$key" in
            enter) break ;;
            esc) edited=; message= choice= footer_override=$ed_saved_footer; return 1 ;;
            backspace) edited=$(printf '%s' "$edited" | sed 's/.$//') ;;
            clear) edited= ;;
            up | down | left | right | other) ;;
            *) [ "${#edited}" -ge 200 ] || edited="$edited$key" ;;
        esac
    done
    edited=$(printf '%s' "$edited" | sed 's/^[[:space:]]*//; s/[[:space:]]*$//')
    message= choice= footer_override=$ed_saved_footer
    return 0
}
select_row() {
    sr_row=$(printf '%s\n' "$rows_out" | grep '^item|' | sed -n "$((sel + 1))p")
    IFS='|' read -r sr_kind id title sr_lic sr_status action <<EOF
$sr_row
EOF
}
# Hand the terminal to Apple, sudo, an agent or a shell, and take it back.
pause_screen() { screen_off; }
resume_screen() { screen_on; }

# ------------------------------------------------------------ work pages ---
work_begin() { # work_begin TITLE SUBTITLE STEP...
    w_screen=$screen w_sel=$sel w_top=$top
    work_title=$1 work_sub=$2; shift 2
    work_labels= work_labels_keys=
    for wb_label in "$@"; do work_add "$wb_label"; done
    screen=work message= choice= footer_override='working · ctrl+c stops' top=0
    draw
}
work_add() {
    work_labels="$work_labels$1
"
    step_key "$1"; work_labels_keys="$work_labels_keys $step_key"
    put "$steps_dir/$step_key" next
    [ "$screen" != work ] || draw
}
# Keys pressed and trackpad scrolls (arrow keys in a terminal) during work
# are not answers: throw away what is waiting, so the menu keeps its row.
drain_keys() {
    stty min 0 time 0 <&3 2>/dev/null || return 0
    dd bs=256 count=64 <&3 >/dev/null 2>&1 || :
    stty min 1 time 0 <&3 2>/dev/null || :
}
# work_end ok|failed MESSAGE: a failure stays on the page, marked on its
# step, until Return or Escape.
work_end() {
    drain_keys
    if [ "$1" != ok ]; then
        footer_override='⏎ back'
        draw
        while :; do key; case "$key" in enter | esc | q) break ;; esac; done
    fi
    footer_override= screen=$w_screen sel=$w_sel top=$w_top message=$2 choice=
}
# The first failed step's reason, for the status line.
work_failure() {
    wf=
    for wf_key in $work_labels_keys; do
        IFS='|' read -r wf_state wf_reason wf_rest < "$steps_dir/$wf_key" 2>/dev/null || continue
        if [ "$wf_state" = failed ]; then wf=$wf_reason; return; fi
    done
}

# ---------------------------------------------------------------- screens ---
# wrap STYLE TEXT: note rows of at most 74 columns.
wrap() {
    printf '%s\n' "$2" | awk -v style="$1" -v r0="$r0" -v w=74 '{
        n = split($0, a, " "); line = ""
        for (i = 1; i <= n; i++) {
            if (line != "" && length(line) + 1 + length(a[i]) > w) { print "note|" style line (style == "" ? "" : r0); line = a[i] }
            else line = (line == "" ? a[i] : line " " a[i])
        }
        if (line != "") print "note|" style line (style == "" ? "" : r0)
    }'
}
signin_rows() {
    printf 'note|\n'
    wrap '' 'Enter the email address you bought your Makepad product with, or have beta access for. Leave it empty for the free apps.'
}
folder_rows() {
    printf 'note|\n'
    wrap '' "The apps' sources, their build output and a private Rust go into one folder of their own. Nothing outside it changes, except Apple's developer tools or your distribution's packages, which the Builder asks about first."
    printf 'note|\n'
    wrap "$dim" 'A folder the Builder used before is reused as it is: its email, apps and choices stay.'
}
gpu_notice='Makepad relies on your GPU to draw its UI and implement AI functionality just like a videogame does. Old hardware and broken drivers can cause your computer to reboot unexpectedly.'
gpu_rows() {
    printf 'note|\n'
    wrap '' "$gpu_notice"
    printf '%s\n' 'item|continue|I understand|||' 'item|cancel|Cancel|||'
}
xcode_rows() {
    printf 'note|\n'
    if [ "$tools_problem" = license ]; then
        wrap '' "Xcode is installed, but its license has not been accepted yet, so Apple's compiler will not run."
        printf 'note|\n'
        wrap "$dim" 'Apple shows the license here in the terminal and asks for your password; type agree at the end to accept it.'
        pad 'Read and accept the Xcode license' 34
        printf 'item|license|%s|||sudo xcodebuild -license\n' "$padded"
    else
        wrap '' "Makepad compiles with Apple's command line developer tools: clang, the macOS SDK and git. They are not installed on this Mac yet."
        printf 'note|\n'
        wrap "$dim" "Apple's installer opens in its own window (about 1 GB); come back here when it has finished."
        pad 'Install the developer tools' 34
        printf "item|install|%s|||opens Apple's installer\n" "$padded"
    fi
    printf '%s\n' 'item|check|Check again|||' 'item|cancel|Cancel|||'
}
packages_rows() {
    printf 'note|\n'
    set -- $packages_cmd
    pr_manager=$1 pr_list= pr_count=0 pr_on=
    for pr_word in "$@"; do
        case "$pr_word" in
            install | -S) pr_on=1 ;;
            -*) ;;
            *) if [ -n "$pr_on" ]; then pr_list="$pr_list $pr_word"; pr_count=$((pr_count + 1)); fi ;;
        esac
    done
    wrap '' "Makepad compiles from source and needs $pr_count development packages from $distro_name. They are installed system-wide by your package manager:"
    printf 'note|\n'
    printf '%s\n' $pr_list | paste -d' ' - - - - - | sed "s/^/note|${dim}  /; s/\$/${r0}/"
    printf 'note|\n'
    printf 'item|install|Install with sudo|||sudo %s\n' "$pr_manager"
    printf 'item|cancel|Cancel|||\n'
}

# ------------------------------------------------------------ agreements ---
agreements='makepad|Makepad commercial license|https://makepad.nl/commercial-license
rust|Rust|https://www.rust-lang.org/policies/licenses'
accepted_agreements() {
    if [ -f "$root/agreements-accepted" ]; then
        accepted=$(tr '\n' ' ' < "$root/agreements-accepted")
    elif [ "${m_rust_ok:-0}" = 1 ] && [ -z "${tools_problem:-}" ]; then
        # Folders set up before this record accepted them when installing.
        accepted='makepad rust'
    else
        accepted=
    fi
}
is_accepted() { case " $accepted " in *" $1 "*) return 0 ;; esac; return 1; }
record_agreements() { put "$root/agreements-accepted" "$(printf '%s\n' "$@")"; accepted="$*"; }
host_of() { host=${1#https://}; host=${host%%/*}; }
open_url() {
    if [ "$plat" = mac ]; then open "$1" >/dev/null 2>&1 || return 1
    else xdg-open "$1" >/dev/null 2>&1 || return 1; fi
}
open_agreement() {
    oa_url=$(printf '%s\n' "$agreements" | awk -F'|' -v id="$1" '$1 == id { print $3 }')
    if open_url "$oa_url"; then message="${ok}✓${r0} Opened ${oa_url#https://} in your browser."
    else message="${warn}Could not open $oa_url${r0}"; fi
}
terms_rows() {
    accepted_agreements
    while IFS='|' read -r tr_id tr_name tr_url; do
        host_of "$tr_url"; pad "$tr_name" 36
        if is_accepted "$tr_id"; then tr_state="${ok}✓${r0} accepted  ${dim}${host}${r0}"; else tr_state="${warn}not accepted  ${r0}${dim}${host}${r0}"; fi
        printf 'item|url:%s|%s||%s|open in browser\n' "$tr_id" "$padded" "$tr_state"
    done <<EOF
$agreements
EOF
    printf '%s\n' 'note|' 'item|agree|Agree to all|||accept every agreement above' 'item|disagree|Disagree|||withdraw acceptance'
}
terms_screen() {
    t_screen=$screen t_sel=$sel t_top=$top
    screen=terms sel=0 top=0 message=
    while :; do
        draw; key; message=
        case "$key" in
            up) sel=$(( (sel + items - 1) % items )) ;;
            down) sel=$(( (sel + 1) % items )) ;;
            esc) break ;;
            q) exit 0 ;;
            enter)
                select_row
                case "$id" in
                    agree) record_agreements makepad rust; message="${ok}✓${r0} Agreements accepted." ;;
                    disagree) record_agreements; message="${warn}Agreements withdrawn: installing or updating build tools will ask again.${r0}" ;;
                    url:*) open_agreement "${id#url:}" ;;
                esac ;;
        esac
    done
    t_message=$message
    screen=$t_screen sel=$t_sel top=$t_top message=$t_message
}
# consent TITLE INTRO DETAIL ACTION ID...: one screen for the terms a step
# needs; Return opens an agreement, "Agree to all" (selected at the start)
# records them, Cancel or Escape backs out. Already accepted: no screen.
consent() {
    accepted_agreements
    c_title=$1 c_intro=$2 c_detail=$3 c_action=$4; shift 4
    c_missing=
    for c_id in "$@"; do is_accepted "$c_id" || c_missing="$c_missing $c_id"; done
    [ -n "$c_missing" ] || return 0
    c_screen=$screen c_sel=$sel c_top=$top c_message=$message
    consent_title=$c_title consent_ids="$*"
    screen=consent top=0 message=
    sel=$#
    c_result=1
    while :; do
        draw; key; message=
        case "$key" in
            up) sel=$(( (sel + items - 1) % items )) ;;
            down) sel=$(( (sel + 1) % items )) ;;
            esc | q) break ;;
            enter)
                select_row
                case "$id" in
                    agree) record_agreements $accepted $c_missing; c_result=0; break ;;
                    cancel) break ;;
                    url:*) open_agreement "${id#url:}" ;;
                esac ;;
        esac
    done
    screen=$c_screen sel=$c_sel top=$c_top message=$c_message
    return "$c_result"
}
consent_rows() {
    printf 'note|\n'
    wrap '' "$c_intro"
    [ -z "$c_detail" ] || printf 'note|%s%s%s\n' "$dim" "$c_detail" "$r0"
    printf 'head|READ THE AGREEMENTS\n'
    while IFS='|' read -r cr_id cr_name cr_url; do
        case " $consent_ids " in *" $cr_id "*) ;; *) continue ;; esac
        host_of "$cr_url"; pad "$cr_name" 36
        printf 'item|url:%s|%s||%s%s%s|open in browser\n' "$cr_id" "$padded" "$dim" "$host" "$r0"
    done <<EOF
$agreements
EOF
    printf 'note|\nitem|agree|Agree to all|||%s\nitem|cancel|Cancel|||\n' "$c_action"
}

# --------------------------------------------------------------- model ---
# scan: every row's state, read from the folder (after each action).
scan() {
    rust_version
    m_rust_ok=0 m_rust_external=
    if [ -n "$rust_version" ] && rust_ready "$rust_version"; then
        m_rust_ok=1
        case "$selected_rust" in /*) m_rust_external=$rust_sysroot ;; esac
    fi
    m_license_rows=
    if [ "$lic_state" = ok ] || [ -z "$email" ]; then sc_ids=$licensed_ids
    else
        sc_ids=
        for sc_file in "$root"/available/*.json; do
            [ -f "$sc_file" ] || continue
            rel_load "$sc_file" 2>/dev/null || continue
            [ "$rel_public" = true ] || sc_ids="$sc_ids $rel_id"
        done
    fi
    for sc_id in $sc_ids; do
        app_state "$sc_id"
        rel_load "$root/available/$sc_id.json" 2>/dev/null || continue
        state_texts "$state" "$rel_bytes"
        if ! rel_supported; then st_text="${dim}not available for this platform yet${r0}" st_act=; fi
        sc_lic=commercial; [ "$rel_license" != beta ] || sc_lic=beta
        m_license_rows="${m_license_rows}item|app:$sc_id|$rel_title|$sc_lic|$st_text|$st_act
"
    done
    m_free_rows= m_free_total=0 m_free_ready=0
    sc_free=$(free_apps)
    while IFS='|' read -r sc_id sc_title; do
        [ -n "$sc_id" ] || continue
        app_state "$sc_id"
        m_free_total=$((m_free_total + 1))
        case "$state" in
            ready) m_free_ready=$((m_free_ready + 1)); sc_st="${ok}✓${r0} ready" sc_act=run ;;
            compile) sc_st='needs compiling' sc_act=compile ;;
            merge) sc_st="${warn}updated · your changes to merge${r0}" sc_act='merge with agent' ;;
            *) sc_st= sc_act=download ;;
        esac
        m_free_rows="${m_free_rows}item|app:$sc_id|$sc_title||$sc_st|$sc_act
"
    done <<EOF
$sc_free
EOF
    m_public_ready=0
    if rel_load "$root/available/makepad.json" 2>/dev/null; then
        rel_dir; m_public_mb=$(( (rel_bytes + 1048575) / 1048576 ))
        ! rel_installed || m_public_ready=1
    fi
}
# The status column says where an app is, in a word or two; the action on
# the selected row says what Return does (each one ends with the app
# running).
state_texts() { # state_texts STATE BYTES -> st_text st_act (menu.txt)
    st_mb=$(( (${2:-0} + 1048575) / 1048576 ))
    case "$1" in
        new) if [ "$st_mb" -gt 0 ]; then st_text="${dim}$st_mb MB${r0}"; else st_text="${dim}not downloaded${r0}"; fi; st_act=download ;;
        partial) st_text="${warn}download stopped${r0}" st_act=resume ;;
        compile) st_text='needs compiling' st_act=compile ;;
        update) st_text="${warn}update available${r0}" st_act=update ;;
        ready) st_text="${ok}✓${r0} ready" st_act=run ;;
        merge) st_text="${warn}updated · your changes to merge${r0}" st_act='merge with agent' ;;
    esac
}
main_rows() {
    # Update sits above everything; the menu still opens on the first app.
    printf 'note|\n'
    case "$checked" in
        '') printf 'item|updates|Update||%scheck for updates%s|=\n' "$dim" "$r0" ;;
        0*) printf 'item|updates|Update||%s✓%s up to date · %s|check again\n' "$ok" "$r0" "${checked#* }" ;;
        *) printf 'item|updates|Update||updates downloaded · %s|check again\n' "${checked#* }" ;;
    esac
    printf 'head|YOUR APPS\n'
    if [ -z "$email" ]; then printf 'item|login|Log in with your email address|||to see your licenses\n'
    else
        case "$lic_state" in
            checking) printf 'note|%schecking licenses for %s…%s\n' "$dim" "$email" "$r0" ;;
            ok) if [ -n "$m_license_rows" ]; then printf '%s' "$m_license_rows"; else printf 'note|%snone on this email yet · buy or request beta access at makepad.nl%s\n' "$dim" "$r0"; fi ;;
            *) printf 'note|%slicenses not checked: %s%s\n' "$warn" "$lic_error" "$r0"; printf '%s' "$m_license_rows" ;;
        esac
    fi
    printf 'head|MAKEPAD EXPERIMENTS\n'
    printf 'item|free|Experiments|free|%s · %s ready|open list\n' "$m_free_total" "$m_free_ready"
    printf 'head|CODING AGENTS · each one knows how to change and rebuild these apps\n'
    [ -z "$agents" ] || printf '%s\n' "$agents" | while IFS='|' read -r mr_cmd mr_title; do printf 'item|agent-%s|%s|||open\n' "$mr_cmd" "$mr_title"; done
    printf "item|agent-shell|Shell||%swith this folder's Rust on PATH%s|open\n" "$dim" "$r0"
    printf 'head|SETUP\n'
    if [ -z "$email" ]; then printf 'item|account|Account||%snot logged in%s|log in\n' "$warn" "$r0"
    else
        case "$lic_state" in
            ok) printf 'item|account|Account||%s✓%s %s|switch or log out\n' "$ok" "$r0" "$email" ;;
            checking) printf 'item|account|Account||%s %schecking…%s|switch or log out\n' "$email" "$dim" "$r0" ;;
            *) printf 'item|account|Account||%s %s· %s%s|switch or log out\n' "$email" "$warn" "$lic_error" "$r0" ;;
        esac
    fi
    if [ "$plat" = linux ]; then
        if [ -f "$root/graphics-notice-read" ]; then printf 'item|gpu|Graphics||%s✓%s driver notice read|\n' "$ok" "$r0"
        else printf 'item|gpu|Graphics||%sread the driver notice%s|read\n' "$warn" "$r0"; fi
    fi
    accepted_agreements
    if is_accepted makepad && is_accepted rust; then printf 'item|terms|Agreements||%s✓%s accepted %s· Makepad, Rust%s|read\n' "$ok" "$r0" "$dim" "$r0"
    else printf 'item|terms|Agreements||%snot accepted%s|read\n' "$warn" "$r0"; fi
    if [ -f "$scratch/disk" ]; then
        read -r mr_total mr_build < "$scratch/disk"
        mr_gb=$(awk -v t="$mr_total" -v b="$mr_build" 'BEGIN { printf "%.1f %.1f", t / 1048576, b / 1048576 }')
        if [ "${mr_build:-0}" -gt 0 ]; then printf 'item|disk|Disk||%s GB used %s· build %s GB%s|clear build data\n' "${mr_gb% *}" "$dim" "${mr_gb#* }" "$r0"
        else printf 'item|disk|Disk||%s GB used|\n' "${mr_gb% *}"; fi
    else
        printf 'item|disk|Disk||%smeasuring…%s|\n' "$dim" "$r0"
    fi
}
free_rows() {
    printf 'note|\n'
    if [ "$m_public_ready" = 1 ]; then printf 'note|%sEach one compiles the first time you run it.%s\n' "$dim" "$r0"
    else printf 'note|%sOne %s MB download covers them all; each compiles on first run.%s\n' "$dim" "${m_public_mb:-0}" "$r0"; fi
    printf '%s' "$m_free_rows"
}
# The index of the item row with ID on the current screen.
select_id() {
    si_n=$(screen_rows | grep '^item|' | grep -n "^item|$1|" | head -n 1 | cut -d: -f1)
    [ -z "$si_n" ] || sel=$((si_n - 1))
}
# The menu starts on the first row of YOUR APPS. awk reads all of main_rows'
# output (a reader that stops early would break its pipe).
first_license_row() {
    sel=$(main_rows | awk '/^head\|YOUR APPS/ { found = 1 } !found && /^item\|/ { n++ } END { print n + 0 }')
}
measure_disk() {
    rm -f "$scratch/disk"
    (
        md_total=$(du -sk "$root" 2>/dev/null | cut -f1)
        md_build=0
        [ ! -d "$root/target" ] || md_build=$(du -sk "$root/target" 2>/dev/null | cut -f1)
        put "$scratch/disk" "${md_total:-0} ${md_build:-0}"
    ) </dev/null >/dev/null 2>&1 &
}

# -------------------------------------------------------------- account ---
# Checked on its own page or row: a refused email comes straight back to the
# editor with the reason; a network failure keeps the email.
licenses_now() {
    lic_state=checking
    draw
    busy "Checking licenses for $email"
    check_licenses || :
    message=
}
# log_out: forget the email in this folder. Only the Makepad experiments
# show from then on; licensed apps already built stay on disk untouched and
# come back when you log in again.
log_out() {
    la_was=$email
    email= licensed_ids= lic_state=none lic_error=
    save_email
    : > "$root/.login-asked"
    : > "$scratch/licensed"
    scan
    message="${ok}✓${r0} Logged out of $la_was. ${dim}Only the Makepad experiments are shown; log in again from the Account row.${r0}"
}
signin_screen() {
    s_screen=${screen:-main}
    screen=signin sel=0 top=0 message=
    si_hint= si_typed=${builder_email:-}
    while :; do
        edit 'Email:' "$si_typed" "$si_hint" 'type your email   ⏎ continue' || { email=; break; }
        [ -n "$edited" ] || { email=${saved_email:-}; break; }
        si_typed=$edited
        if ! check_email "$edited"; then si_hint="${warn}That does not look like an email address.${r0}"; continue; fi
        email=$email_ok
        licenses_now
        if [ "$lic_state" = rejected ]; then
            si_hint="${warn}Email not recognised: $si_typed · fix any typo, then ⏎${r0}"
            email=
            continue
        fi
        break
    done
    screen=$s_screen message=
}
switch_account() {
    if [ -n "$email" ]; then
        choose "Logged in as $email." '' 'switch email' 'switch email' 'log out' || return 0
        if [ "$chosen" = 'log out' ]; then log_out; return 0; fi
        edit 'Email for your licenses:' '' "${dim}⏎ keeps $email${r0}" '⏎ log in   esc back' || return 0
    else
        edit 'Email you bought a Makepad app with:' '' '' '⏎ log in   esc back' || return 0
    fi
    [ -n "$edited" ] && [ "$edited" != "$email" ] || return 0
    check_email "$edited" || { message="${warn}That does not look like an email address.${r0}"; return 0; }
    sa_before=$email sa_typed=$edited
    email=$email_ok
    while :; do
        licenses_now
        [ "$lic_state" = rejected ] || break
        edit 'Email for your licenses:' "$sa_typed" "${warn}Email not recognised: $sa_typed · fix any typo, then ⏎${r0}" '⏎ check again   esc keep' || {
            email=$sa_before; licenses_now; scan; return 0
        }
        sa_typed=$edited
        check_email "$edited" || continue
        email=$email_ok
    done
    save_email
    : > "$root/.login-asked"
    scan
    license_count
    sa_s=s; [ "$license_count" != 1 ] || sa_s=
    if [ "$lic_state" = ok ]; then message="${ok}✓${r0} Switched to $email · $license_count license$sa_s."
    else message="${warn}Switched to $email; the licenses could not be checked: $lic_error${r0}"; fi
}

# --------------------------------------------------------------- folder ---
# first_folder: asks where to install, reuses a folder the Builder made, and
# copies this script there as the `makepad` command.
folder_screen() {
    screen=folder sel=0 top=0 message=
    fs_default=${MAKEPAD_LOADER_ROOT:-$HOME/makepad-commercial}
    fs_home=$(cd -P -- "$HOME" && pwd -P)
    while :; do
        choose "Install into $(short "$fs_default")?" '' install install 'other folder' quit || exit 0
        fs_folder=$fs_default
        case "$chosen" in
            quit) exit 0 ;;
            'other folder')
                edit 'Folder:' "$(short "$fs_default")" '' '⏎ use this folder   esc back' || continue
                fs_folder=$edited ;;
        esac
        case "$fs_folder" in
            '~') fs_folder=$HOME ;;
            '~/'*) fs_folder=$HOME/${fs_folder#'~/'} ;;
            /*) ;;
            *) fs_folder=$(pwd -P)/$fs_folder ;;
        esac
        if mkdir -p -- "$fs_folder" 2>/dev/null && fs_root=$(cd -P -- "$fs_folder" && pwd -P); then
            if [ "$fs_root" = / ] || [ "$fs_root" = "$fs_home" ]; then
                message="${warn}Please choose a folder of its own, such as $(short "$HOME/makepad-commercial").${r0}"
            elif [ -e "$fs_root/builder/makepad-builder.json" ] || [ -e "$fs_root/makepad-builder.json" ] || [ -e "$fs_root/makepad-loader.json" ] || [ -z "$(ls -A "$fs_root")" ]; then
                home=$fs_root root=$fs_root/builder
                migrate_layout
                break
            else
                message="${warn}$(short "$fs_root") already holds other files; please choose an empty or new folder.${r0}"
            fi
        else
            message="${warn}Could not create $(short "$fs_folder"); please choose another folder.${r0}"
        fi
        fs_default=$fs_folder
    done
    message=
}
install_self() {
    cp "$self" "$home/makepad.next"
    chmod 755 "$home/makepad.next"
    mv -f "$home/makepad.next" "$home/makepad"
}

# ------------------------------------------------------------- graphics ---
gpu_screen() { # 0 when read
    g_screen=$screen g_sel=$sel
    screen=gpu sel=0 top=0 message=
    g_ok=1
    while :; do
        draw; key
        case "$key" in
            up) sel=0 ;;
            down) sel=1 ;;
            enter) [ "$sel" != 0 ] || g_ok=0; break ;;
            esc | q) break ;;
        esac
    done
    screen=$g_screen sel=$g_sel
    [ "$g_ok" = 0 ] || return 1
    put "$root/graphics-notice-read" read
}

# ----------------------------------------------------------- build tools ---
# choose_rust VERSION: the one-time offer of a compatible installed Rust, or
# repairing a recorded one that stopped validating. Private is the default.
choose_rust() {
    recorded_rust
    cr_stale=
    case "$selected_rust" in
        private) return 0 ;;
        /*) rust_check --validate "$1" "$selected_rust" && return 0; cr_stale=$selected_rust; log "The selected Rust at $selected_rust cannot be used: $rust_reason" ;;
    esac
    if rust_check --probe "$1"; then
        cr_candidate=$rust_sysroot
        choose "Build with a private Rust $1 in this folder, or your system Rust?" "system: $(short "$cr_candidate") (not modified)" private private system || return 1
        if [ "$chosen" = system ]; then record_rust "$cr_candidate"; log 'Using the installed Rust.'; else record_rust private; log 'Using a private Rust in this folder.'; fi
    elif [ -n "$cr_stale" ]; then
        choose "The selected Rust at $(short "$cr_stale") cannot be used. Switch to a private Rust in this folder?" '' switch switch keep || return 1
        [ "$chosen" = switch ] || { message="${warn}Kept the selected Rust at $(short "$cr_stale"). Make it available again, or select the app again to switch.${r0}"; return 1; }
        record_rust private
    else
        log "Installed Rust not used: $rust_reason"
        record_rust private
    fi
}
change_rust() {
    rust_version
    if ! rust_check --probe "$rust_version"; then
        message="${dim}No other compatible installed Rust was found: $rust_reason${r0}"
        return 0
    fi
    cr_candidate=$rust_sysroot
    recorded_rust
    cr_pick=private; case "$selected_rust" in /*) cr_pick=system ;; esac
    choose "Build with a private Rust $rust_version in this folder, or your system Rust?" "system: $(short "$cr_candidate") (not modified)" "$cr_pick" private system || return 0
    if [ "$chosen" = system ]; then record_rust "$cr_candidate"; else record_rust private; fi
    scan
    if [ "$m_rust_ok" = 1 ]; then message="${ok}✓${r0} Rust changed; the next compile uses it."; else install_build_tools || :; fi
}
# xcode_screen: Apple's installer or its license prompt, Check again or
# Cancel. 0 when the tools are ready or Apple's installer was started.
xcode_screen() {
    x_screen=$screen x_sel=$sel
    screen=xcode sel=0 top=0 message=
    x_result=1
    while :; do
        draw; key; message=
        case "$key" in
            up) sel=$(( (sel + items - 1) % items )) ;;
            down) sel=$(( (sel + 1) % items )) ;;
            esc | q) break ;;
            enter)
                select_row
                case "$id" in
                    install) /usr/bin/xcode-select --install >/dev/null 2>&1 || :; x_result=0; xcode_waiting=1; break ;;
                    license)
                        pause_screen
                        printf "\n  Apple's Xcode license follows. Type agree at the end to accept it.\n\n"
                        sudo xcodebuild -license </dev/tty || :
                        resume_screen
                        if tools_check; then x_result=0; break; fi
                        message="${warn}Still not ready.${r0}" ;;
                    check)
                        busy 'Checking clang, the macOS SDK, the linker and git'
                        if tools_check; then x_result=0; break; fi
                        message="${warn}Still not ready.${r0}" ;;
                    cancel) break ;;
                esac ;;
        esac
    done
    screen=$x_screen sel=$x_sel
    return "$x_result"
}
packages_screen() { # 0 when sudo is ready to install the packages
    p_screen=$screen p_sel=$sel
    screen=packages sel=0 top=0 message=
    p_result=1
    while :; do
        draw; key
        case "$key" in
            up) sel=0 ;;
            down) sel=1 ;;
            enter) [ "$sel" != 0 ] || p_result=0; break ;;
            esc | q) break ;;
        esac
    done
    screen=$p_screen sel=$p_sel
    [ "$p_result" = 0 ] || return 1
    # sudo asks for the password on the real terminal; the packages then
    # install beside the Rust download.
    pause_screen
    printf '\n  Makepad installs development packages from %s with:\n\n    sudo %s\n\n' "$distro_name" "$packages_cmd"
    if sudo -v </dev/tty; then resume_screen; return 0; fi
    resume_screen
    message="${warn}sudo did not run; nothing was installed.${r0}"
    return 1
}
job_xcode() { # waits for Apple's installer, checking every few seconds
    while ! tools_check; do
        step "$1" "now|0|0|none|0|waiting for Apple's installer · esc stops waiting"
        sleep 3
    done
    step "$1" 'done|clang, SDK, git'
}
job_packages() {
    step "$1" "now|0|0|none|0|installing with ${packages_cmd%% *}"
    # shellcheck disable=SC2086
    if ! sudo -n $packages_cmd >> "$log_file" 2>&1; then step_fail "$1" 'The package manager failed (see builder.log)'; return 1; fi
    tools_check || { step_fail "$1" 'Some development packages are still missing (see builder.log)'; return 1; }
    step "$1" 'done|compiler, linker, git'
}
# install_build_tools: what compiling needs, as the Windows Builder does it:
# one consent screen, then every missing piece side by side on one page.
install_build_tools() {
    rust_version
    if [ -z "$rust_version" ]; then
        busy 'Checking the Makepad release'
        fetch_public || { message="${warn}The Makepad release could not be checked: $public_error${r0}"; return 1; }
        rust_version
    fi
    ib_version=$rust_version
    ib_tools=0; tools_check && ib_tools=1
    ib_rust=0; rust_ready "$ib_version" && ib_rust=1
    if [ "$ib_rust" = 0 ]; then
        choose_rust "$ib_version" || { message="${dim}Nothing was installed.${r0}"; return 1; }
        rust_ready "$ib_version" && ib_rust=1
    fi
    [ "$ib_tools" = 0 ] || [ "$ib_rust" = 0 ] || return 0
    if [ "$ib_tools" = 1 ]; then ib_intro='Makepad compiles from source, so it needs a compiler.'
    elif [ "$plat" = mac ]; then ib_intro="Makepad compiles from source, so it needs a compiler. Apple's developer tools come from Apple's installer, which shows its own license."
    else ib_intro="Makepad compiles from source, so it needs a compiler. Development packages come from your distribution's package manager."; fi
    ib_detail=; [ "$ib_rust" = 1 ] || ib_detail="Installed in this folder only: Rust $ib_version"
    ib_action='install Rust'; [ "$ib_tools" = 1 ] || ib_action='install the build tools and Rust'
    [ "$ib_rust" = 0 ] || ib_action='install the build tools'
    consent 'Install build tools' "$ib_intro" "$ib_detail" "$ib_action" makepad rust || { message="${dim}Nothing was installed.${r0}"; return 1; }
    ib_steps= xcode_waiting=
    if [ "$ib_tools" = 0 ]; then
        if [ "$plat" = mac ]; then
            xcode_screen || { message="${dim}Nothing was installed.${r0}"; return 1; }
            [ -z "$xcode_waiting" ] || ib_steps='Xcode tools'
        else
            [ "$tools_problem" != unsupported ] || { message="${warn}The Builder needs glibc Linux (x86_64 or ARM64).${r0}"; return 1; }
            linux_packages || { message="${warn}Install the development packages with your package manager, then choose System packages.${r0}"; return 1; }
            packages_screen || { message="${message:-${dim}Nothing was installed.${r0}}"; return 1; }
            ib_steps='System packages'
        fi
    fi
    [ "$ib_rust" = 1 ] || ib_steps="$ib_steps${ib_steps:+|}Rust"
    if [ -n "$ib_steps" ]; then
        IFS='|'; set -- $ib_steps; IFS=$ifs_default
        work_begin 'Install build tools' 'This takes a few minutes; everything goes into this folder.' "$@"
        for ib_step in "$@"; do
            case "$ib_step" in
                'Xcode tools') job_start job_xcode 'Xcode tools'; xcode_job=${running_jobs##* } ;;
                'System packages') job_start job_packages 'System packages' ;;
                Rust) job_start job_rust Rust "$ib_version" ;;
            esac
        done
        if jobs_wait; then
            work_end ok "${ok}✓${r0} Rust $ib_version is ready. Caches and builds stay in $(short "$root")."
        else
            work_failure
            work_end failed "${warn}${wf:-Setup stopped.}${r0}"
            scan; tools_check || :
            return 1
        fi
    fi
    tools_check || :
    measure_disk
    scan
}

# ------------------------------------------------------------------ apps ---
# open_app ID: merge saved changes, run a built app, or download, compile
# and run it on its work page, as every row does in the Windows Builder.
open_app() {
    oa_id=$1
    app_state "$oa_id"
    if [ "$state" = merge ]; then merge_changes "$oa_id"; return; fi
    if [ "$state" = ready ]; then
        rel_load "$root/installed/$oa_id.json" && rel_dir
        busy "Opening $rel_title"
        launch_app
        launched_message "$rel_title"
        return
    fi
    install_build_tools || return 0
    app_release_file "$oa_id" || release_file="$root/available/makepad.json"
    rel_load "$release_file" || { message="${warn}$oa_id has no usable release; choose Update.${r0}"; return 0; }
    rel_supported || { message="${warn}$rel_title is not available for this platform yet.${r0}"; return 0; }
    if [ "$rel_public" != true ]; then
        [ -n "$email" ] || { message="${warn}Log in with the email that has this app's license.${r0}"; return 0; }
        email_headers
    fi
    oa_title=$rel_title
    [ "$rel_id" != makepad ] || oa_title=$(free_apps | awk -F'|' -v id="$oa_id" '$1 == id { print $2 }')
    rust_ready "$rel_rust" || { message="${warn}The latest source requires Rust $rel_rust; it could not be set up.${r0}"; return 0; }
    keep_edits "$oa_id" "$release_file"
    rel_load "$release_file"
    source_labels
    oa_steps=$(printf '%s' "$source_labels")
    oa_sub='Your app is compiling from source, it will start when completed.'
    IFS=$nl
    # shellcheck disable=SC2086
    set -- $oa_steps Compile Open
    IFS=$ifs_default
    work_begin "$oa_title" "$oa_sub" "$@"
    log "$(describe_sources)"
    if [ -n "$oa_steps" ]; then
        checkout_jobs
        if ! jobs_wait; then work_failure; work_end failed "${warn}${wf:-The download stopped.}${r0}"; scan; return 0; fi
    fi
    # A free app's package comes from the registry of the source it builds.
    if [ "$rel_id" = makepad ]; then
        load_registry
        app_release_file "$oa_id" || { step_fail Compile "$oa_id is not in this Makepad release"; work_end failed; return 0; }
        rel_load "$release_file"
    fi
    if [ "$release_file" = "$root/tmp/projected-$oa_id.json" ]; then
        mkdir -p "$root/available"; cp "$release_file" "$root/available/$oa_id.json"; release_file="$root/available/$oa_id.json"
    fi
    rel_dir
    prepare_target
    job_start job_compile Compile "$oa_id"
    if ! jobs_wait; then work_failure; work_end failed "${warn}${wf:-The compile stopped.}${r0}"; scan; return 0; fi
    step Open 'now|0|0|none|0|opening'
    work_redraw
    rel_load "$root/installed/$oa_id.json" && rel_dir
    launch_app
    oa_title=$rel_title
    step Open 'done|running'
    work_redraw
    prune_snapshots || :
    checked="0 $(date +%H:%M)"
    measure_disk
    launched_message "$oa_title"
    work_end ok "$message"
    if [ -f "$root/changes/$oa_id.merge" ]; then message="${warn}$oa_title is updated. Your edits are in changes/; select it to merge.${r0}"; fi
    scan
}
# One line for the log saying where the sources come from, so a full
# recompile can be told apart from a shared build.
describe_sources() {
    rel_dir
    if [ "$rel_directory" = "$root/sources/$rel_release" ]; then printf 'Sources for %s: %s' "$rel_title" "$rel_directory"
    else printf 'Sources for %s shared with %s (same commits, same build artifacts)' "$rel_title" "$rel_directory"; fi
}

merge_changes() { # merge_changes APP
    mc_app=$1
    mc_diff=$(cat "$root/changes/$mc_app.merge" 2>/dev/null || :)
    mc_agents=$(printf '%s\n' "$agents" | grep -v '^grok|' | grep . || :)
    mc_count=$(printf '%s\n' "$mc_agents" | grep -c . || :)
    case "$mc_count" in
        0) message="${warn}Install Claude Code or Codex to merge, or apply changes/$mc_diff yourself.${r0}"; return 0 ;;
        1) mc_agent=${mc_agents%%|*} ;;
        *)
            set -- $(printf '%s\n' "$mc_agents" | cut -d'|' -f1)
            choose 'Merge your changes with which agent?' '' "$1" "$@" || return 0
            mc_agent=$chosen ;;
    esac
    mc_title=$(printf '%s\n' "$agents" | awk -F'|' -v id="$mc_agent" '$1 == id { print $2 }')
    launch_agent "$mc_agent" "Reapply my changes saved in changes/$mc_diff in this folder onto the new $mc_app source. Keep the intent of each change where the new release moved code, rebuild it with ./makepad build $mc_app until it compiles, and report any change that no longer applies." || return 0
    rm -f "$root/changes/$mc_app.merge"
    message="${ok}✓${r0} $mc_title finished merging. Select the app to run it."
    scan
}
# real_tty: the terminal's own device (/dev/ttys003), for programs started in
# it. Handed /dev/tty, Claude Code (Bun) stops at once on macOS with "EINVAL:
# invalid argument, kqueue", since kqueue cannot watch that alias.
real_tty() {
    rt_name=$(ps -o tty= -p $$ 2>/dev/null | tr -d ' ')
    case "$rt_name" in
        '' | '?' | '??') real_tty=/dev/tty ;;
        /dev/*) real_tty=$rt_name ;;
        *) real_tty=/dev/$rt_name ;;
    esac
    [ -c "$real_tty" ] && [ -r "$real_tty" ] && [ -w "$real_tty" ] || real_tty=/dev/tty
}
# launch_agent NAME [TASK]: the agent in this folder, knowing how to change
# and rebuild the apps, with this folder's Rust on PATH.
launch_agent() {
    agent_release || { message="${warn}Download an app first; coding agents work in its source.${r0}"; return 1; }
    rust_ready "$rel_rust" && tools_check || { message="${warn}Set up the build tools first, so the agent can rebuild apps.${r0}"; return 1; }
    write_agent_files
    la_title=$(printf '%s\n' "$agents" | awk -F'|' -v id="$1" '$1 == id { print $2 }')
    real_tty
    pause_screen
    # The agent gets the terminal as it is, with its own UI. Keys pressed
    # before it is up are dropped, so a second Return cannot answer its first
    # question (Claude Code's "trust this folder?" has "No, exit" selected).
    printf '\033[H\033[2J%s\n\n' "Starting ${la_title:-$1} in $(short "$root")…"
    stty -icanon min 0 time 0 </dev/tty 2>/dev/null && dd bs=4096 count=1 </dev/tty >/dev/null 2>&1 || :
    stty "$tty_saved" <&3 2>/dev/null || :
    la_started=$(now)
    la_status=0
    (
        cargo_env "$rel_label" "$rel_id" >/dev/null
        export MAKEPAD_AGENT_CONTEXT="$root/agent-context.txt"
        cd "$root"
        case "$1" in
            claude) if [ -n "${2:-}" ]; then exec claude --append-system-prompt "$agent_directive" "$2"; else exec claude --append-system-prompt "$agent_directive"; fi ;;
            codex) if [ -n "${2:-}" ]; then exec codex -c "developer_instructions='$agent_directive'" "$2"; else exec codex -c "developer_instructions='$agent_directive'"; fi ;;
            *) exec "$1" ;;
        esac
    ) <"$real_tty" >"$real_tty" 2>&1 || la_status=$?
    resume_screen
    # Closed straight away: most often its first question (trust this
    # folder?) was answered No, which also exits 1.
    if [ $(($(now) - la_started)) -lt 8 ]; then
        message="${warn}${la_title:-$1} closed right away.${r0} ${dim}If it asked whether to trust this folder, open it again and choose Yes.${r0}"
        return 1
    fi
    if [ "$la_status" != 0 ]; then message="${warn}${la_title:-$1} exited $la_status. It must already be installed and signed in.${r0}"; return 1; fi
    scan
}
open_shell() {
    real_tty
    pause_screen
    printf '\nProject: %s\nThis folder'"'"'s Rust is on PATH. Type exit to return to Makepad.\n\n' "$home"
    (
        if agent_release && rust_ready "$rel_rust"; then
            cargo_env "$rel_label" "$rel_id" >/dev/null
            write_agent_files
            export MAKEPAD_AGENT_CONTEXT="$root/agent-context.txt"
        fi
        cd "$home"
        exec "${SHELL:-/bin/sh}"
    ) <"$real_tty" >"$real_tty" 2>&1 || :
    resume_screen
    scan
}

# ---------------------------------------------------------------- update ---
# Licenses first (new purchases appear, expired ones go), then every
# installed app is compared with its newest release: unchanged sources are
# replaced by a clean download; edited ones are first saved as a diff.
check_updates() {
    consent Update 'Updates download the newest sources of your apps.' '' 'check for updates' makepad || { message="${dim}Nothing was downloaded.${r0}"; return 0; }
    licenses_now
    work_begin Update 'Newer sources replace unchanged ones; your edits are kept as a diff.' 'Check for updates'
    step 'Check for updates' 'now|0|0|none|0|checking'
    work_redraw
    if ! fetch_public; then step_fail 'Check for updates' "$public_error"; work_end failed "${warn}$public_error${r0}"; return 0; fi
    load_registry
    for cu_file in "$root"/available/*.json "$root"/installed/*.json; do
        [ -f "$cu_file" ] || continue
        cu_id=${cu_file##*/}; cu_id=${cu_id%.json}
        [ "$cu_id" != makepad ] || continue
        is_free "$cu_id" || continue
        project_public "$cu_id" "$root/available/$cu_id.json" || :
    done
    step 'Check for updates' "done|licenses and releases checked"
    work_redraw
    cu_updated= cu_merges= cu_started=
    for cu_file in "$root"/installed/*.json; do
        [ -f "$cu_file" ] || continue
        cu_id=${cu_file##*/}; cu_id=${cu_id%.json}
        rel_load "$cu_file" 2>/dev/null || continue
        cu_have=$rel_release
        rel_load "$root/available/$cu_id.json" 2>/dev/null || continue
        [ "$rel_release" != "$cu_have" ] && rel_supported || continue
        if [ "$rel_public" != true ]; then case " $licensed_ids " in *" $cu_id "*) ;; *) continue ;; esac; fi
        keep_edits "$cu_id" "$root/available/$cu_id.json"
        [ ! -f "$root/changes/$cu_id.merge" ] || cu_merges=1
        rel_load "$root/available/$cu_id.json"
        release_file="$root/available/$cu_id.json"
        [ "$rel_public" = true ] || email_headers
        rel_dir
        cu_key="$rel_directory|$rel_repos"
        case "$cu_started" in *"<$cu_key>"*) cu_updated="$cu_updated${cu_updated:+, }$rel_title"; continue ;; esac
        cu_started="$cu_started<$cu_key>"
        source_labels
        while IFS= read -r cu_label; do [ -z "$cu_label" ] || work_add "$cu_label"; done <<EOF
$source_labels
EOF
        checkout_jobs
        cu_updated="$cu_updated${cu_updated:+, }$rel_title"
    done
    if ! jobs_wait; then work_failure; work_end failed "${warn}${wf:-The update stopped.}${r0}"; scan; return 0; fi
    measure_disk
    if [ -z "$cu_updated" ]; then checked="0 $(date +%H:%M)"; work_end ok "${ok}✓${r0} Licenses checked; everything is up to date."
    elif [ -n "$cu_merges" ]; then checked="1 $(date +%H:%M)"; work_end ok "${ok}✓${r0} Updated $cu_updated. Your edits are saved in changes/; select to merge."
    else checked="1 $(date +%H:%M)"; work_end ok "${ok}✓${r0} Updated $cu_updated. Each compiles on its next run."; fi
    scan
}
clear_build() {
    [ -d "$root/target" ] || { message="${dim}There is no build data to delete.${r0}"; return 0; }
    choose 'Delete the build data in target/?' 'your apps keep working; the next compile starts from scratch' keep keep delete || return 0
    [ "$chosen" = delete ] || return 0
    busy 'Deleting build data'
    if remove_inside "$root/target"; then message="${ok}✓${r0} Build data cleaned. Built apps are unchanged."
    else message="${warn}Could not delete all build data; the activity log says why.${r0}"; fi
    measure_disk
    scan
}

# ---------------------------------------------------------------- start ---
agents=
for ag in 'claude|Claude Code' 'codex|Codex' 'grok|Grok'; do
    if command -v "${ag%%|*}" >/dev/null 2>&1; then agents="$agents${agents:+
}$ag"; fi
done
lic_state=none lic_error= licensed_ids= checked= tools_problem= m_rust_ok=0 m_license_rows= m_free_rows= m_free_total=0 m_free_ready=0
screen=main sel=0 top=0

# Run again through curl | sh (not from the folder's own `makepad`): an
# installation already in the default folder is opened as it is, with its
# saved email, and its `makepad` command becomes this version. Only a folder
# without one is a first run.
if [ -z "$root" ]; then
    rr_home=${MAKEPAD_LOADER_ROOT:-$HOME/makepad-commercial}
    if [ -f "$rr_home/builder/makepad-builder.json" ] || [ -f "$rr_home/makepad-builder.json" ] || [ -f "$rr_home/makepad-loader.json" ]; then
        home=$(cd -P -- "$rr_home" && pwd -P) root=$home/builder
        migrate_layout
        install_self
    fi
fi
if [ -z "$root" ]; then
    # First run: the email, then the folder, then Rust; each once.
    scratch=$(mktemp -d "${TMPDIR:-/tmp}/makepad-builder.XXXXXX")
    steps_dir="$scratch/steps" slots="$scratch/slots" log_file="$scratch/builder.log"
    mkdir -p "$steps_dir" "$slots"
    saved_email=
    for fr_root in "${MAKEPAD_LOADER_ROOT:-$HOME/makepad-commercial}/builder" "${MAKEPAD_LOADER_ROOT:-$HOME/makepad-commercial}"; do
        [ -f "$fr_root/makepad-builder.json" ] || continue
        root=$fr_root; read_email; saved_email=$email; root=
        break
    done
    email=
    signin_screen
    first_email=$email first_state=$lic_state
    folder_screen
    screen=main
    install_self
    [ -n "$first_email" ] || { read_email; first_email=$email; first_state=; }
    session
    email=$first_email
    save_email
    : > "$root/.login-asked"
    log "Makepad Builder installed in $home"
else
    session
    if [ -z "$email" ] && [ ! -f "$root/.login-asked" ]; then
        signin_screen
        save_email
        : > "$root/.login-asked"
    fi
fi
if [ "$plat" = linux ] && [ ! -f "$root/graphics-notice-read" ]; then
    gpu_screen || { quiet_exit='Setup cancelled. Nothing was downloaded or installed.'; exit 0; }
fi

# Every start refreshes the licenses, then stops at the menu with the first
# app selected.
[ -f "$root/available/makepad.json" ] || { busy 'Checking the Makepad release'; fetch_public || log "$public_error"; }
load_registry
tools_check || :
scan
draw
if [ -n "$email" ]; then
    licenses_now
    if [ "$lic_state" = rejected ]; then
        sw_before=$email
        switch_typed=$email
        while [ "$lic_state" = rejected ]; do
            edit 'Email for your licenses:' "$switch_typed" "${warn}Email not recognised: $switch_typed · fix any typo, then ⏎${r0}" '⏎ check again   esc keep' || break
            switch_typed=$edited
            check_email "$edited" || continue
            email=$email_ok
            licenses_now
        done
        [ "$lic_state" != ok ] || save_email
    fi
fi
measure_disk
scan
draw
# Build tools are set up when an app is first built, not at the start.
# Start on the first licensed app, or on logging in.
sel=0
first_license_row

while :; do
    polling=0; [ -f "$scratch/disk" ] || polling=1
    draw
    key
    [ "$key" = none ] && continue
    message=
    case "$key" in
        up) [ "$items" = 0 ] || sel=$(( (sel + items - 1) % items )) ;;
        down) [ "$items" = 0 ] || sel=$(( (sel + 1) % items )) ;;
        q) exit 0 ;;
        esc) if [ "$screen" = main ]; then exit 0; else screen=main sel=$free_back top=0; fi ;;
        enter)
            select_row
            case "$screen/$id" in
                main/account | main/login) switch_account ;;
                main/gpu) gpu_screen || : ;;
                main/terms) terms_screen ;;
                main/rust) if [ "$m_rust_ok" = 1 ]; then change_rust; else install_build_tools || :; fi ;;
                main/system)
                    busy 'Checking the system developer tools'
                    if tools_check; then message="${ok}✓${r0} Compiler, SDK, linker and git are ready."; else install_build_tools || :; fi
                    scan ;;
                main/disk) clear_build ;;
                main/updates) check_updates ;;
                main/free) free_back=$sel screen=free sel=${free_sel:-0} top=0 ;;
                main/agent-shell) open_shell ;;
                main/agent-*) launch_agent "${id#agent-}" || : ;;
                main/app:* | free/app:*)
                    [ "$screen" != free ] || free_sel=$sel
                    open_app "${id#app:}" ;;
            esac ;;
    esac
done
MAKEPAD_BUILDER
    chmod 700 "$makepad_dir/makepad"
    if [ "$#" = 0 ]; then
        # The Builder in this terminal (its keys come from the terminal, not
        # from the pipe this script arrived through); it copies itself into
        # the folder you choose.
        exec sh "$makepad_dir/makepad" </dev/null
    fi
    # A command (the Rust Builder's read-only probes): run it and tidy up.
    makepad_status=0
    sh "$makepad_dir/makepad" "$@" || makepad_status=$?
    rm -f "$makepad_dir/makepad"
    rmdir "$makepad_dir" 2>/dev/null || :
    return "$makepad_status"
}

makepad_builder "$@"
