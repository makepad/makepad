#!/bin/sh
# Makepad commercial app installer and launcher for macOS: one shell script,
# nothing compiled for itself. Rust is installed only to build the apps.
#
# PREVIEW: the backend is simulated (no downloads, no writes). The screens,
# keys and flow are the real design. Coding agents are really detected.
#
# Every run is a restart: the script reads what earlier runs left in the
# folder and continues from there, so re-downloading and re-running it is
# always safe. It refreshes your licenses whenever it opens.
#
#   sh makepad-mac.sh                 first run
#   sh makepad-mac.sh --installed     an existing installation
#   sh makepad-mac.sh --resume        an earlier run stopped mid-download
#   sh makepad-mac.sh --no-rust       no system Rust found
#   sh makepad-mac.sh --free          an email without licenses
#   sh makepad-mac.sh --email a@b.c
#   sh makepad-mac.sh --linux         the same script on Linux: GPU notice + packages
#   sh makepad-mac.sh --no-xcode      macOS without Apple's developer tools
#   sh makepad-mac.sh --xcode-license Xcode installed, license not accepted
#
# The installed copy (~/makepad-commercial/makepad) is also the command
# coding agents use; without a terminal UI it takes a command:
#   makepad build APP | run APP | list | env | agent-context
set -eu

demo=first demo_rust=found demo_free=no email= command= plat=mac gpu=ok pkgs=ok xcode=ok
while [ $# -gt 0 ]; do
    case "$1" in
        --installed) demo=installed email=rik2@n4.io ;;
        --resume) demo=resume email=rik2@n4.io ;;
        --no-rust) demo_rust=none ;;
        --linux) plat=linux ;;
        --no-xcode) xcode=missing ;;
        --xcode-license) xcode=license ;;
        --free) demo_free=yes ;;
        --email) shift; email=$1 ;;
        -h|--help) sed -n '2,24p' "$0"; exit 0 ;;
        build|run|list|env|agent-context) command=$1; shift; break ;;
        *) printf 'unknown option %s\n' "$1" >&2; exit 1 ;;
    esac
    shift
done

# ------------------------------------------------------------------ model ---
# App states, all derived from the folder on every start:
#   new       nothing downloaded             -> download, compile, run
#   partial   a download was interrupted      -> resume, compile, run
#   compile   source is newer than the build  -> compile, run
#   ready     built from the current source   -> run
# The live version reads these markers:
#   makepad.json                  the account email
#   selected-rust                 "local" or the system sysroot
#   toolchain/rust/<v>/.ready     local Rust completely unpacked
#   downloads/<repo>.part         an interrupted download (curl -C - resumes)
#   sources/<repo>/.verified      source checked against the catalog sha256
#   <Title>.app/.../build-source  the source commit the bundle was built from
#   .lock/pid                     another run is active (stale ones cleared)
root="$HOME/makepad-commercial"
rust_pin=1.92.0
system_rust=1.93.1
[ "$demo_rust" = found ] || system_rust=
licensed=unknown
checked=
message=

get() { eval "printf '%s' \"\${st_$1:-new}\""; }
put() { eval "st_$1=\$2"; }

# Licensed apps come from the VPS catalog: id|title|license|megabytes|source
fetch_licenses() {
    if [ "$demo_free" = yes ]; then licenses=; else licenses='scope|Makepad Scope|commercial|214|sources/scope
amp|Makepad Amp|beta|188|sources/stage/apps/amp'; fi
}
# The free apps in the public Makepad repository (apps.json in the live one).
free_apps='wm|Makepad WM
mail|Mail
notes|Notes
calendar|Calendar
terminal|Terminal
files|Files
photos|Photos
sheets|Sheets
pdf|PDF
mixer|Mixer
clock|Clock
weather|Weather
calculator|Calculator
reminders|Reminders
finance|Finance
video|Video
browser|Browser
fab|Fab
director|Director
route|Route'

scan() {
    rust=local
    case "$demo" in
        first) rust=none ;;
        installed) put scope ready; put amp ready; put makepad ready; put wm ready; put mail ready ;;
        resume) put scope partial ;;
    esac
    # Free apps share one Makepad download; built ones are ready, the rest
    # compile on first run.
    src=$(get makepad)
    for id in $(printf '%s\n' "$free_apps" | cut -d'|' -f1); do
        if [ "$src" = new ]; then put "$id" new; elif [ "$(get "$id")" != ready ]; then put "$id" compile; fi
    done
}

# ------------------------------------------------------- agent interface ---
# The environment every build uses, the TUI's and an agent's alike: the
# selected compiler first on PATH, caches and output inside the folder.
print_env() {
    if [ "${rust:-local}" = system ]; then bin="$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin"; else bin="$root/toolchain/rust/$rust_pin/bin"; fi
    cat <<EOF
export PATH="$bin:\$PATH"
export CARGO_HOME="$root/cargo-home"
export CARGO_TARGET_DIR="$root/target"
export RUSTUP_TOOLCHAIN= RUSTC_WRAPPER= RUSTFLAGS=
# One shared target/ lets every app reuse the crates the others compiled.
# No debug info or separate debug files; stripped binaries.
export CARGO_PROFILE_RELEASE_DEBUG=0 CARGO_PROFILE_RELEASE_SPLIT_DEBUGINFO=off CARGO_PROFILE_RELEASE_STRIP=true
EOF
}

# Written to AGENTS.md (and CLAUDE.md) in the folder before an agent opens,
# so Claude, Codex and Grok all read it on start.
print_agent_context() {
    cat <<EOF
# Makepad apps in this folder

Everything here is Makepad applications as Rust source. The person using this
folder wants you to change these apps and rebuild them.

## Build and run

Always use the \`makepad\` command in this folder; do not call cargo directly.
It uses this folder's compiler (a private Rust $rust_pin in toolchain/, or the
system Rust recorded in selected-rust), keeps caches and build output in this
folder, and rebuilds the macOS app bundle so Dock icons keep working.

    ./makepad build <app>     compile after you change its source
    ./makepad run <app>       compile if needed, then open <App>.app
    ./makepad list            every app, its source folder and state

A build prints compiler errors in full; fix them and build again. If you need
cargo itself (tests, clippy), first run: eval "\$(./makepad env)"

## Apps

| app | name | source |
|-----|------|--------|
EOF
    printf '%s\n' "$licenses" | while IFS='|' read -r id title _ _ src; do
        printf '| %s | %s | %s |\n' "$id" "$title" "$src"
    done
    printf '%s\n' "$free_apps" | while IFS='|' read -r id title; do
        printf '| %s | %s | sources/makepad/apps/%s |\n' "$id" "$title" "$id"
    done
    cat <<EOF

Makepad itself (widgets, platform, shaders) is in sources/makepad; read its
AGENTS.md before changing it. Each repository keeps its own instructions.

## After an update

Updates never merge. When an app you changed gets a new release, the old
changes are saved first and the app is replaced by the clean new source:

    changes/<app>-<date>.diff    unified diff against the release you edited
    changes/<app>-<date>.files   one "M|A|D path" line per changed file

When asked to merge, reapply that diff onto the new source by editing files
(do not rely on git being installed), keep the intent of each change where the
new release moved code, then ./makepad build <app> until it compiles. Report
any change that no longer applies.

## Rules

- Keep edits inside sources/. Built bundles (<App>.app) are regenerated.
- Your licenses do not allow publishing the commercial sources.
EOF
}

if [ -n "$command" ]; then
    fetch_licenses
    rust=local
    case "$command" in
        env) print_env ;;
        agent-context) print_agent_context ;;
        list) { printf '%s\n' "$licenses" | cut -d'|' -f1,2,5; printf '%s\n' "$free_apps" | sed 's#$#|sources/makepad/apps#'; } | awk -F'|' '{ printf "%-12s %-14s %s\n", $1, $2, $3 }' ;;
        build|run)
            app=${1:?usage: makepad $command APP}
            # Plain, line-based output for agents: every compiler message,
            # then one result line they can check.
            printf 'Compiling %s with Rust %s (preview: simulated)\n' "$app" "$rust_pin"
            printf 'Bundled %s/%s.app\n' "$root" "$app"
            [ "$command" = build ] || printf 'Opened %s.app\n' "$app" ;;
    esac
    exit 0
fi

# Coding agents: only what is installed on this machine.
agents=
for a in 'claude|Claude Code' 'codex|Codex' 'grok|Grok'; do
    if command -v "${a%%|*}" >/dev/null 2>&1; then agents="$agents${agents:+
}$a"; fi
done

# --------------------------------------------------------------- terminal ---
exec 3<>/dev/tty || { printf 'Makepad needs an interactive terminal.\n' >&2; exit 1; }
e=$(printf '\033')
dim="$e[2m" b="$e[1m" ok="$e[32m" acc="$e[36m" warn="$e[33m" inv="$e[7m" r0="$e[0m"
tty_saved=$(stty -g <&3)
leave() {
    stty "$tty_saved" <&3 2>/dev/null || :
    printf '%s[?25h%s[?1049l' "$e" "$e"
}
trap leave EXIT
trap 'exit 130' INT TERM HUP
printf '%s[?1049h%s[?25l' "$e" "$e"
stty -icanon -echo min 1 time 0 <&3

short() { case "$1" in "$HOME"/*) printf '~/%s' "${1#"$HOME"/}" ;; *) printf '%s' "$1" ;; esac; }
at() { printf '%s[%d;1H%s[2K' "$e" "$1" "$e"; }
line() { printf '%s%s[K\n' "$*" "$e"; y=$((y + 1)); }
size() {
    width=$(tput cols 2>/dev/null || printf 80)
    height=$(tput lines 2>/dev/null || printf 24)
    [ "$width" -le 80 ] || width=80
}
rule() { r=; i=4; while [ "$i" -lt "$width" ]; do r="${r}─"; i=$((i + 1)); done; printf '  %s%s%s' "$dim" "$r" "$r0"; }

key() {
    # The trailing x keeps a newline (Return) distinguishable from end of
    # input; a closed terminal quits instead of pressing Return forever.
    k=$(dd bs=1 count=1 <&3 2>/dev/null; printf x)
    k=${k%x}
    case "$k" in
        '') exit 1 ;;
        "$e")
            stty min 0 time 1 <&3
            s=$(dd bs=2 count=1 <&3 2>/dev/null)
            stty min 1 time 0 <&3
            case "$s" in '[A'|OA) key=up ;; '[B'|OB) key=down ;; '[C'|OC) key=right ;; '[D'|OD) key=left ;; '') key=esc ;; *) key=other ;; esac ;;
        '
'|' ') key=enter ;;
        k) key=up ;;
        j) key=down ;;
        *) key=$k ;;
    esac
}

# ------------------------------------------------------------------ rows ---
# A screen is a list of rows: "head|TITLE", "note|TEXT" or
# "item|id|name|license|status|action".
state_cells() { # state_cells ID -> st (status text) and act (action)
    case "$(get "$1")" in
        new) st="${dim}download 42 MB, compile and run$r0" act== ;;
        partial) st="${warn}download stopped at 38% · resume, compile and run$r0" act== ;;
        compile) st="${warn}compile and run$r0" act== ;;
        ready) st="${ok}✓$r0 ready" act=run ;;
        merge) st="${warn}updated · your changes to merge$r0" act='merge with agent' ;;
    esac
}
license_rows() {
    printf '%s\n' "$licenses" | while IFS='|' read -r id title lic mb _; do
        state_cells "$id"
        [ "$(get "$id")" != new ] || st="${dim}download $mb MB, compile and run$r0"
        printf 'item|%s|%s|%s|%s|%s\n' "$id" "$title" "$lic" "$st" "$act"
    done
}
free_rows() {
    # One shared download covers every app here, so rows stay short: a mark
    # when built, and the action only on the selected row.
    if [ "$(get makepad)" = new ]; then
        printf '%s\n' "note|" "note|${dim}One 42 MB download covers them all; each compiles on first run.$r0"
    else
        printf '%s\n' "note|" "note|${dim}Each one compiles the first time you run it.$r0"
    fi
    printf '%s\n' "$free_apps" | while IFS='|' read -r id title; do
        case "$(get "$id")" in
            ready) st="${ok}✓$r0 ready" act=run ;;
            new) st= act='download source, compile and run' ;;
            *) st= act='compile and run' ;;
        esac
        printf 'item|%s|%s||%s|%s\n' "$id" "$title" "$st" "$act"
    done
}
agent_rows() {
    if [ -n "$agents" ]; then
        printf '%s\n' "$agents" | while IFS='|' read -r id title; do
            printf 'item|agent-%s|%s||%s|open\n' "$id" "$title" ""
        done
    fi
    printf 'item|agent-shell|Shell||%s|open\n' "${dim}with this folder's Rust on PATH$r0"
}
# Disk: what this folder uses. The live version sums `du -sk` of target/
# (build data), sources/ + changes/ (source), toolchain/ (Rust) and the
# built <App>.app bundles (apps); the preview derives the split from the app
# states. Clearing build data deletes target/ only: the apps keep running,
# the next compile starts from scratch.
disk_row() {
    kb_build=0 kb_src=0 kb_rust=0 kb_apps=0
    [ "$rust" != local ] || kb_rust=820000
    for id in scope amp makepad; do
        case "$(get "$id")" in new) ;; *) kb_src=$((kb_src + 240000)) ;; esac
    done
    for id in scope amp $(printf '%s\n' "$free_apps" | cut -d'|' -f1); do
        if [ "$(get "$id")" = ready ]; then
            kb_apps=$((kb_apps + 30000))
            [ "${cleared:-}" = yes ] || kb_build=$((kb_build + 380000))
        fi
    done
    total=$((kb_build + kb_src + kb_rust + kb_apps))
    free_kb=$(df -k "$HOME" | awk 'NR == 2 { print $4 }')
    n() { awk -v k="$1" 'BEGIN { printf "%.1f", k / 1048576 }'; }
    # What the folder uses, and the part a clean build can delete (target/).
    # Sources, the toolchain and built apps stay.
    if [ "$kb_build" -eq 0 ]; then
        printf 'item|disk|Disk||%s|\n' "$(n "$total") GB used"
        return
    fi
    printf 'item|disk|Disk||%s|%s\n' "$(n "$total") GB used ${dim}· build $(n "$kb_build") GB$r0" 'clean build'
}

clear_build() {
    [ "${cleared:-}" != yes ] || return 0
    choose "Delete the build data in $(short "$root")/target? Your apps keep working; the next compile starts from scratch." keep delete keep || return 0
    [ "$chosen" = delete ] || return 0
    spin 'Deleting build data' 0.6
    cleared=yes
    message="${ok}✓$r0 Build data deleted. Built apps are unchanged."
}

gpu_row() {
    [ "$plat" = linux ] || return 0
    gs="${ok}✓$r0 driver notice read" ga=
    [ "$gpu" = ok ] || gs="${warn}read the driver notice$r0" ga=read
    printf '%s' "item|gpu|Graphics||$gs|$ga"
}
tools_row() {
    if [ "$plat" != linux ]; then
        case "$xcode" in
            ok) printf '%s' "item|tools|Xcode tools||${ok}✓$r0 clang, SDK, git|recheck" ;;
            missing) printf '%s' "item|tools|Xcode tools||${warn}not installed$r0|install" ;;
            license) printf '%s' "item|tools|Xcode tools||${warn}license not accepted$r0|accept" ;;
        esac
        return
    fi
    n=$(printf '%s\n' $packages | wc -l | tr -d ' ')
    ps="${ok}✓$r0 $distro development packages" pa=recheck
    [ "$pkgs" = ok ] || ps="${warn}$n missing$r0" pa=install
    printf '%s' "item|tools|System packages||$ps|$pa"
}
# The email lives only in this folder (makepad.json): the download carries
# none, and a reinstall into the same folder remembers it.
account_row() {
    if [ -n "$email" ]; then printf '%s' "item|account|Account||$email|switch email"
    else printf '%s' "item|account|Account||${warn}not logged in$r0|log in"; fi
}
main_rows() {
    case "$rust" in
        none) rs="${warn}not set up$r0" ra='set up' ;;
        installing) rs="${acc}installing…$r0" ra= ;;
        local) rs="${ok}✓$r0 local $rust_pin ${dim}in $(short "$root")$r0" ra=change ;;
        system) rs="${ok}✓$r0 system $system_rust" ra=change ;;
    esac
    ready=0 total=0
    for id in $(printf '%s\n' "$free_apps" | cut -d'|' -f1); do
        total=$((total + 1))
        [ "$(get "$id")" != ready ] || ready=$((ready + 1))
    done
    printf '%s\n' "head|SETUP" \
        "$(account_row)" \
        "$(gpu_row)" \
        "item|terms|Agreements||$(if [ "$accepted" = yes ]; then printf '%s' "${ok}✓$r0 accepted ${dim}· Makepad, Rust$r0"; else printf '%s' "${dim}Makepad, Rust$r0"; fi)|read" \
        "item|rust|Rust||$rs|$ra" \
        "$(tools_row)" \
        "$(disk_row)" \
        "$(if [ -z "$checked" ]; then printf '%s' "item|updates|Update||${dim}check for updates$r0|="; else printf '%s' "item|updates|Update||$checked|check again"; fi)" \
        "head|YOUR LICENSES"
    case "$licensed" in
        unknown) printf '%s\n' "note|${dim}checking licenses for ${email}…$r0" ;;
        *) if [ -z "$email" ]; then printf '%s\n' "item|login|Log in with your email address|||to see your licenses"; elif [ -n "$licenses" ]; then license_rows; else printf '%s\n' "note|${dim}none on this email yet · buy or request beta access at makepad.nl$r0"; fi ;;
    esac
    printf '%s\n' "head|MAKEPAD EXPERIMENTS" \
        "item|free|Experiments|free|$total · $ready ready|open list" \
        "head|CODING AGENTS · each one knows how to change and rebuild these apps"
    agent_rows
}

# ---------------------------------------------------------------- screen ---
draw() {
    size
    case "$screen" in
        main) rows_out=$(main_rows | grep -v '^$'); crumb= sub='Makepad apps ship as source, so your own coding agent can customize them.' ;;
        free) rows_out=$(free_rows); crumb=' › Makepad experiments' sub='From our open source repository: experiments, not finished applications.' ;;
        terms) rows_out=$(terms_rows); crumb=' › License agreements' sub='Return opens an agreement in your browser.' ;;
        signin) rows_out=$(signin_rows); crumb=' › Log in' sub='Welcome to the Makepad Builder.' ;;
        xcode) rows_out=$(xcode_rows); crumb=' › Apple developer tools' sub='Needed to compile on macOS.' ;;
        gpu) rows_out=$(gpu_rows); crumb=' › Graphics' sub='Before you continue.' ;;
        consent) rows_out=$(consent_rows); crumb=" › $consent_title" sub='Please read what you are agreeing to.' ;;
        packages) rows_out=$(packages_rows); crumb=' › System packages' sub='Installed outside this folder, so please check them.' ;;
    esac
    items=$(printf '%s\n' "$rows_out" | grep -c '^item|' || :)
    y=0
    printf '%s[H' "$e"
    line
    left="Makepad commercial apps$crumb"
    shown_email=${email:-not logged in}
    pad=$((width - 4 - ${#left} - ${#shown_email}))
    [ "$pad" -gt 1 ] || pad=1
    line "  ${b}Makepad$r0 commercial apps$crumb$(printf '%*s' "$pad" '')$dim${email:-not logged in}$r0"
    line "  $dim$sub$r0"
    # Long lists scroll: keep the selection inside the visible window.
    room=$((height - 11))
    [ "$room" -ge 3 ] || room=3
    [ "$sel" -ge "$top" ] || top=$sel
    [ "$sel" -lt $((top + room)) ] || top=$((sel - room + 1))
    [ "$screen" != main ] || top=0
    i=0 shown=0
    while IFS='|' read -r kind id title lic st act; do
        case "$kind" in
            head) line; line "    $dim$id$r0"; continue ;;
            note) line "    $id"; continue ;;
        esac
        if [ "$i" -lt "$top" ] || { [ "$screen" != main ] && [ "$shown" -ge "$room" ]; }; then i=$((i + 1)); continue; fi
        [ "$shown" -gt 0 ] || [ "$screen" = main ] || [ "$screen" = gpu ] || [ "$screen" = packages ] || [ "$screen" = consent ] || [ "$screen" = xcode ] || line
        # Pad plain text; escape codes must not count toward column widths.
        name=$(printf '%-15s' "$title")
        case "$lic" in
            commercial) lt="$b$(printf '%-20s' 'commercial license')$r0" ;;
            beta) lt="$warn$(printf '%-20s' 'beta access')$r0" ;;
            free) lt="$dim$(printf '%-20s' 'free, open source')$r0" ;;
            *) lt= ;;
        esac
        if [ "$i" = "$sel" ]; then
            # act "=" means the status already says what Return does.
            hint=; [ -z "$act" ] || hint="  $acc$act ⏎$r0"; [ "$act" != = ] || hint=" ${acc}⏎$r0"
            # With no status the action sits in the status column itself.
            [ -n "$st" ] || hint="${hint#  }"
            line "  ${acc}›$r0 $b$name$r0 $lt$st$hint"
        else
            line "    $name $lt$st"
        fi
        i=$((i + 1)) shown=$((shown + 1))
    done <<ROWS
$rows_out
ROWS
    if [ "$screen" != main ]; then
        more=$((items - top - shown))
        if [ "$more" -gt 0 ]; then line "    ${dim}↓ $more more$r0"; else line; fi
    fi
    line
    line "$(rule)"
    status_row=$((y + 1))
    line "  $message"
    line
    if [ "$screen" = signin ]; then
        line "  ${dim}type your email   ⏎ continue$r0"
    elif [ "$screen" = gpu ] || [ "$screen" = packages ] || [ "$screen" = consent ] || [ "$screen" = xcode ]; then
        line "  ${dim}↑↓ move   ⏎ select   esc cancel$r0"
    elif [ "$screen" = main ]; then
        line "  ${dim}↑↓ move   ⏎ select   q quit$r0"
    else
        line "  ${dim}↑↓ move   ⏎ select   esc back   q quit$r0"
    fi
    printf '%s[J' "$e"
}

status() { at "$status_row"; printf '  %s' "$*"; }

# While work runs, the key hints give way to a busy line; the next draw
# puts them back.
busy() { at $((status_row + 2)); printf '  %s' "${dim}working · ctrl+c stops$r0"; }

bar() { # bar LABEL FROM% TOTAL UNIT SECONDS TEXT
    busy
    label=$1 from=$2 total=$3 unit=$4 secs=$5 text=$6 steps=40
    i=$((from * steps / 100))
    while [ "$i" -le "$steps" ]; do
        fill=$((i * 30 / steps)) s= t= k=0
        while [ "$k" -lt 30 ]; do if [ "$k" -lt "$fill" ]; then s="${s}━"; else t="${t}─"; fi; k=$((k + 1)); done
        n=$(awk -v t="$total" -v i="$i" -v n="$steps" 'BEGIN { v = t * i / n; if (t >= 100) printf "%d", v; else printf "%.1f", v }')
        status "$(printf '%-12s' "$label") $acc$s$dim$t$r0 $(printf '%3d%%' $((i * 100 / steps)))  $dim$n / $total $unit · $text$r0"
        sleep "$(awk -v s="$secs" -v n="$steps" 'BEGIN { printf "%.3f", s / n }')"
        i=$((i + 1))
    done
}

spin() { # spin TEXT SECONDS
    busy
    i=0 n=$(awk -v s="$2" 'BEGIN { printf "%d", s * 12 }')
    while [ "$i" -lt "$n" ]; do
        case $((i % 10)) in 0) c=⠋ ;; 1) c=⠙ ;; 2) c=⠹ ;; 3) c=⠸ ;; 4) c=⠼ ;; 5) c=⠴ ;; 6) c=⠦ ;; 7) c=⠧ ;; 8) c=⠇ ;; *) c=⠏ ;; esac
        status "$acc$c$r0 $1"
        sleep 0.08
        i=$((i + 1))
    done
}

# choose QUESTION DEFAULT OPTION...: ←→ or a first letter picks, ⏎ accepts,
# $note, when set, is a dim remark after the options (cleared on return).
# Escape backs out.
choose() {
    q=$1 pick=$2; shift 2
    while :; do
        o=
        for opt in "$@"; do
            if [ "$opt" = "$pick" ]; then o="$o $inv $opt $r0"; else o="$o  $opt "; fi
        done
        status "$q"
        at $((status_row + 1)); printf '  %s  %s' "$o" "$dim${note:-}$r0"
        key
        case "$key" in
            enter) chosen=$pick; at $((status_row + 1)); note=; return 0 ;;
            esc|q) chosen=; at $((status_row + 1)); note=; return 1 ;;
            left|right)
                prev= next= found=
                for opt in "$@"; do
                    if [ -n "$found" ] && [ -z "$next" ]; then next=$opt; fi
                    [ "$opt" != "$pick" ] || found=1
                    [ -n "$found" ] || prev=$opt
                done
                if [ "$key" = left ] && [ -n "$prev" ]; then pick=$prev; fi
                if [ "$key" = right ] && [ -n "$next" ]; then pick=$next; fi ;;
            *) for opt in "$@"; do case "$(printf '%s' "$opt" | tr '[:upper:]' '[:lower:]')" in "$key"*) chosen=$opt; at $((status_row + 1)); note=; return 0 ;; esac; done ;;
        esac
    done
}

# edit PROMPT: a normal line editor on the status line; empty keeps nothing.
edit() {
    status "$1 "
    stty "$tty_saved" <&3
    printf '%s[?25h' "$e"
    IFS= read -r edited <&3 || edited=
    printf '%s[?25l' "$e"
    stty -icanon -echo min 1 time 0 <&3
}

# ---------------------------------------------------------------- actions ---
# First screen of a first run: the email that holds the licenses. It is kept
# in this folder only (makepad.json), so a rerun or reinstall here skips this.
signin_rows() {
    printf '%s\n' "note|" \
        "note|Enter the email address you bought a Makepad app with, such as Scope" \
        "note|or Amp, or that has beta access. It shows your licenses and stays in" \
        "note|this folder only." \
        "note|" \
        "note|${dim}Leave it empty to continue with the open source experiments; you can" \
        "note|${dim}log in later from the menu.$r0"
}
signin() {
    s_screen=$screen s_sel=$sel
    screen=signin
    while :; do
        draw
        edit "Email:"
        case "$edited" in
            '') break ;;
            *@*.*) email=$edited; break ;;
            *) message="${warn}That does not look like an email address.$r0" ;;
        esac
    done
    message= screen=$s_screen sel=$s_sel
}

switch_account() {
    if [ -n "$email" ]; then edit "Email for your licenses ${dim}(⏎ keeps $email)$r0:"; else edit "Email you bought a Makepad app with:"; fi
    case "$edited" in
        '') return ;;
        *@*.*) ;;
        *) message="${warn}That does not look like an email address.$r0"; return ;;
    esac
    email=$edited
    refresh
    n=0; [ -z "$licenses" ] || n=$(printf '%s\n' "$licenses" | wc -l | tr -d ' ')
    message="${ok}✓$r0 Switched to $email · $n licenses."
}

# License agreements: one list, one acceptance. Return on a row opens it in
# the browser. Apple's developer tools carry their own license, which Apple's
# installer shows; it is not repeated here.
makepad_license_url=https://makepad.nl/commercial-license
agreements="makepad|Makepad commercial license|$makepad_license_url
rust|Rust licenses|https://www.rust-lang.org/policies/licenses"
accepted=no

terms_rows() {
    printf '%s\n' "$agreements" | while IFS='|' read -r id title url; do
        host=${url#https://}; host=${host%%/*}
        printf 'item|%s|%-36s||%s|open in browser\n' "$id" "$title" "$dim$host$r0"
    done
}

# The agreements screen, entered from the SETUP row or from the acceptance
# question; Escape returns to wherever it was opened from.
terms_loop() {
    t_screen=$screen t_sel=$sel t_top=$top t_message=$message
    screen=terms sel=0 top=0 message=
    while :; do
        draw
        key
        message=
        case "$key" in
            up) [ "$sel" -eq 0 ] || sel=$((sel - 1)) ;;
            down) [ "$sel" -ge $((items - 1)) ] || sel=$((sel + 1)) ;;
            esc|q) break ;;
            enter)
                url=$(printf '%s\n' "$agreements" | sed -n "$((sel + 1))p" | cut -d'|' -f3)
                open "$url" 2>/dev/null || :
                message="${ok}✓$r0 Opened ${url#https://} in your browser." ;;
        esac
    done
    screen=$t_screen sel=$t_sel top=$t_top message=$t_message
}

# One question covers every agreement; accept is the default.
# consent TITLE INTRO DETAIL ID...: a screen of its own for the license terms
# a step needs. The agreements are listed by name, Return opens one in the
# browser; "Agree to all" (selected at the start) and Cancel close the list.
consent() {
    c_screen=$screen c_sel=$sel c_message=$message
    consent_title=$1 consent_intro=$2 consent_detail=$3; shift 3
    consent_ids="$*"
    screen=consent message=
    # Start on "Agree to all", below the agreements.
    sel=$(($# + 0))
    while :; do
        draw
        key
        message=
        case "$key" in
            up) [ "$sel" -eq 0 ] || sel=$((sel - 1)) ;;
            down) [ "$sel" -ge $((items - 1)) ] || sel=$((sel + 1)) ;;
            esc|q) id=cancel ;;
            enter) select_row ;;
            *) continue ;;
        esac
        case "$key/$id" in
            */agree) screen=$c_screen sel=$c_sel message=$c_message; draw; return 0 ;;
            */cancel) screen=$c_screen sel=$c_sel message=$c_message; return 1 ;;
            enter/*)
                url=$(printf '%s\n' "$agreements" | awk -F'|' -v id="$id" '$1 == id { print $3 }')
                open "$url" 2>/dev/null || :
                message="${ok}✓$r0 Opened ${url#https://} in your browser." ;;
        esac
    done
}

consent_rows() {
    printf '%s\n' "note|" "note|$consent_intro" "note|$dim$consent_detail$r0" "head|READ THE AGREEMENTS"
    printf '%s\n' "$agreements" | while IFS='|' read -r id title url; do
        case " $consent_ids " in *" $id "*) ;; *) continue ;; esac
        host=${url#https://}; host=${host%%/*}
        printf 'item|%s|%-36s||%s|open in browser\n' "$id" "$title" "$dim$host$r0"
    done
    printf '%s\n' "note|" "item|agree|Agree to all|||$consent_what" "item|cancel|Cancel|||"
}

# First run: the Makepad and Rust terms, on the same screen Windows uses.
accept_terms() {
    [ "$accepted" != yes ] || return 0
    consent_what='agree and continue'
    consent 'License agreements' "Makepad apps are licensed to you, and they are compiled with Rust." "Rust $rust_pin is installed in this folder only if you choose a local Rust." makepad rust || return 1
    accepted=yes
}

# Linux only: the GPU notice (a screen before anything is probed, downloaded
# or compiled) and the distribution's development packages, which are the
# one thing installed outside this folder, system-wide through sudo.
setup_gpu() {
    g_screen=$screen g_sel=$sel g_message=$message
    screen=gpu sel=0 message=
    while :; do
        draw
        key
        case "$key" in
            up) sel=0 ;;
            down) sel=1 ;;
            enter) [ "$sel" -eq 0 ] && gpu=ok; break ;;
            esc|q) sel=1; break ;;
        esac
    done
    r_sel=$sel
    screen=$g_screen sel=$g_sel message=$g_message
    [ "$r_sel" -eq 0 ]
}
gpu_rows() {
    printf '%s\n' "note|" \
        "note|Makepad heavily relies on your GPU to draw its UI and implement AI" \
        "note|functionality. Old hardware and broken drivers can cause your computer" \
        "note|to reboot unexpectedly." \
        "note|" \
        "item|continue|I understand|||" \
        "item|cancel|Cancel|||"
}

distro='Ubuntu 24.04'
packages='build-essential clang cmake pkg-config git curl libssl-dev libx11-dev libxcursor-dev libxkbcommon-dev libxrandr-dev libxi-dev libasound2-dev libpulse-dev libwayland-dev libegl-dev libgl-dev libdrm-dev libgbm-dev'
setup_packages() {
    p_screen=$screen p_sel=$sel p_message=$message
    screen=packages sel=0 message=
    while :; do
        draw
        key
        case "$key" in
            up) sel=0 ;;
            down) sel=1 ;;
            enter) break ;;
            esc|q) sel=1; break ;;
        esac
    done
    r_sel=$sel
    screen=$p_screen sel=$p_sel message=$p_message
    draw
    [ "$r_sel" -eq 0 ] || return 1
    # The live version leaves this screen for sudo's password prompt, then
    # turns the package manager's output into this one bar.
    spin 'sudo apt-get install (asks for your password)' 0.8
    bar Packages 0 "$(printf '%s\n' $packages | wc -l | tr -d ' ')" packages 2.5 apt-get
    pkgs=ok
    message="${ok}✓$r0 Development packages installed."
}
packages_rows() {
    n=$(printf '%s\n' $packages | wc -l | tr -d ' ')
    printf '%s\n' "note|" \
        "note|Makepad compiles from source and needs $n development packages from" \
        "note|$distro. They are installed system-wide by your package manager:" \
        "note|"
    printf '%s\n' $packages | paste -d' ' - - - - | while IFS= read -r l; do
        printf 'note|%s\n' "$dim  $l$r0"
    done
    printf '%s\n' "note|" \
        "item|install|Install with sudo|||sudo apt-get install" \
        "item|cancel|Cancel|||"
}

# macOS: Apple's command line developer tools (clang, the macOS SDK, git).
# Makepad never installs or accepts them itself: it opens Apple's installer,
# or hands the terminal to Apple's own license prompt, then checks again.
xcode_rows() {
    case "$xcode" in
        missing) printf '%s\n' "note|" \
            "note|Makepad compiles with Apple's command line developer tools: clang," \
            "note|the macOS SDK and git. They are not installed on this Mac yet." \
            "note|" \
            "note|${dim}Apple's installer opens in its own window (about 1 GB); come back$r0" \
            "note|${dim}here when it has finished.$r0" \
            "note|" \
            "item|install|$(printf '%-34s' 'Install the developer tools')|||opens Apple's installer" \
            "item|check|Check again|||" \
            "item|cancel|Cancel|||" ;;
        license) printf '%s\n' "note|" \
            "note|Xcode is installed, but its license has not been accepted yet, so" \
            "note|Apple's compiler will not run." \
            "note|" \
            "note|${dim}Apple shows the license here in the terminal and asks for your$r0" \
            "note|${dim}password; type agree at the end to accept it.$r0" \
            "note|" \
            "item|license|$(printf '%-34s' 'Read and accept the Xcode license')|||sudo xcodebuild -license" \
            "item|check|Check again|||" \
            "item|cancel|Cancel|||" ;;
    esac
}
setup_xcode() {
    [ "$xcode" != ok ] || return 0
    x_screen=$screen x_sel=$sel x_message=$message
    screen=xcode sel=0 message=
    while [ "$xcode" != ok ]; do
        draw
        key
        message=
        case "$key" in
            up) [ "$sel" -eq 0 ] || sel=$((sel - 1)) ;;
            down) [ "$sel" -ge $((items - 1)) ] || sel=$((sel + 1)) ;;
            esc|q) break ;;
            enter)
                select_row
                case "$id" in
                    install)
                        # Live: xcode-select --install, then poll the check.
                        spin "Waiting for Apple's installer to finish" 3
                        xcode=ok ;;
                    license)
                        # Live: leave the screen, run sudo xcodebuild -license
                        # with the real terminal, then redraw and check.
                        spin "Apple's license prompt runs here (simulated)" 2
                        xcode=ok ;;
                    check)
                        spin 'Checking clang, the macOS SDK, the linker and git' 0.8
                        message="${warn}Still not ready.$r0" ;;
                    cancel) break ;;
                esac ;;
        esac
    done
    screen=$x_screen sel=$x_sel message=$x_message
    if [ "$xcode" = ok ]; then draw; message="${ok}✓$r0 Apple developer tools are ready."; return 0; fi
    return 1
}

setup_rust() {
    if [ -n "$system_rust" ]; then
        choose "Use your system Rust $system_rust, or a local $rust_pin that keeps your system clean?" local system local || return 0
        if [ "$chosen" = system ]; then
            rust=system
            message="${ok}✓$r0 Using your system Rust $system_rust. Caches and builds stay in $(short "$root")."
            return
        fi
    else
        choose "Install a local Rust $rust_pin into $(short "$root")? It keeps your system clean." install install cancel || return 0
        [ "$chosen" = install ] || return 0
    fi
    rust=installing; draw
    bar Rust 0 66.4 MB 2.5 static.rust-lang.org
    spin 'Checking Rust' 0.4
    rust=local
    message="${ok}✓$r0 Local Rust $rust_pin installed. Nothing outside $(short "$root") changed."
}

# Download what is missing, compile when the build is older than the source,
# bundle, then run: every app goes through the same steps from whatever state
# it is in. The result is a real <Title>.app (icon, Info.plist, the binary
# inside), opened with `open`, so its Dock icon can be kept.
open_app() { # open_app ID TITLE MB CRATES
    id=$1 title=$2 mb=$3 crates=$4
    if [ "$plat" = mac ] && [ "$xcode" != ok ]; then setup_xcode || return 0; fi
    if [ "$rust" = none ]; then setup_rust; [ "$rust" != none ] || return 0; fi
    case "$(get "$id")" in
        new) bar "$title" 0 "$mb" MB 2.5 'from makepad.nl'; put "$id" compile ;;
        partial) bar "$title" 38 "$mb" MB 1.8 resuming; put "$id" compile ;;
    esac
    if [ "$(get "$id")" = compile ]; then
        spin "Verifying $title source" 0.4
        bar "$title" 0 "$crates" crates 3 compiling
        spin "Bundling $title.app" 0.3
        put "$id" ready
    fi
    spin "Opening $title.app" 0.5
    if [ "$plat" = linux ]; then message="${ok}✓$r0 $title is running. ${dim}It is also in your app launcher.$r0"; else message="${ok}✓$r0 $title.app is running. ${dim}Right-click its Dock icon › Options › Keep in Dock.$r0"; fi
}

open_free() { # a free app: the shared Makepad download first, then its build
    id=$1 title=$2
    if [ "$(get makepad)" = new ]; then
        if [ "$rust" = none ]; then setup_rust; [ "$rust" != none ] || return 0; fi
        bar Makepad 0 41.7 MB 2 'open source apps, shared by all of them'
        put makepad ready
        for f in $(printf '%s\n' "$free_apps" | cut -d'|' -f1); do [ "$(get "$f")" != new ] || put "$f" compile; done
    fi
    open_app "$id" "$title" 0 96
}

open_agent() { # open_agent TITLE [TASK]
    if [ "$rust" = none ]; then setup_rust; [ "$rust" != none ] || return 0; fi
    # The live version writes AGENTS.md + CLAUDE.md from print_agent_context,
    # leaves this screen, runs the agent in the folder with print_env applied
    # (TASK as its first prompt), and redraws when the agent exits.
    spin "Writing AGENTS.md for $1" 0.4
    message="${ok}✓$r0 $1 would open in $(short "$root") knowing ./makepad build and run. ${dim}Preview: sh makepad-mac.sh agent-context$r0"
    [ -z "${2:-}" ] || message="${ok}✓$r0 $1 would open with: ${dim}$2$r0"
}

pick_agent() { # sets agent to the one installed agent, or asks which
    agent=
    n=$(printf '%s\n' "$agents" | grep -c . || :)
    case "$n" in
        0) message="${warn}Install Claude Code, Codex or Grok to merge your changes, or apply the diff yourself.$r0"; return 1 ;;
        1) agent=$(printf '%s\n' "$agents" | cut -d'|' -f2); return 0 ;;
    esac
    set -- $(printf '%s\n' "$agents" | cut -d'|' -f1)
    choose "Merge your changes with which agent?" "$1" "$@" || return 1
    agent=$(printf '%s\n' "$agents" | awk -F'|' -v id="$chosen" '$1 == id { print $2 }')
}

# After an update, local edits are never merged by the installer. Your edited
# tree was diffed against the release it came from (git on macOS/Linux,
# makepad-git in the Windows Builder, so no git is needed there), the diff was
# saved in changes/, and the app now holds the clean new release. Your coding
# agent reapplies the diff; it reads it like any file.
merge_changes() { # merge_changes ID TITLE
    pick_agent || return 0
    diff="changes/$1-$(date +%Y-%m-%d).diff"
    open_agent "$agent" "Reapply my changes in $diff onto the new $2 source, then ./makepad build $1."
    put "$1" compile
}

# Updates: licenses first (new purchases appear, expired ones go), then every
# downloaded repository is compared with the newest release on makepad.nl.
# An untouched repository is replaced by a clean fetch of the new release. A
# repository you changed is first saved as a diff against its release:
#   changes/<app>-<date>.diff    unified diff, new files included
#   changes/<app>-<date>.files   one "M|A|D path" line per changed file
# then replaced the same way, and the app waits for your agent to merge.
check_updates() {
    refresh
    got= kept=
    if [ "$demo" = installed ] && [ "$(get amp)" = ready ]; then
        spin 'Makepad Amp has local changes: saving them as a diff' 0.6
        bar 'Makepad Amp' 0 188 MB 2 'clean source for the new release'
        put amp merge
        got='Makepad Amp' kept=' Your 3 changed files are saved in changes/; select Makepad Amp to merge them.'
    fi
    if [ "$demo" = installed ] && [ "$(get makepad)" = ready ]; then
        bar Makepad 0 41.7 MB 1.5 'clean source for the new release'
        put makepad updated
        for f in $(printf '%s\n' "$free_apps" | cut -d'|' -f1); do [ "$(get "$f")" != ready ] || put "$f" compile; done
        got="$got${got:+ and }Makepad apps"
    fi
    checked="${ok}✓$r0 up to date · $(date +%H:%M)"
    if [ -n "$got" ]; then
        checked="updates downloaded · $(date +%H:%M)"
        message="${ok}✓$r0 Updated $got.$kept"
    else
        message="${ok}✓$r0 Licenses checked; everything is up to date."
    fi
}

refresh() {
    if [ -z "$email" ]; then licenses= licensed=yes; return; fi
    licensed=unknown
    draw
    spin "Checking licenses for $email" 0.8
    fetch_licenses
    licensed=yes
}

select_row() {
    row=$(printf '%s\n' "$rows_out" | grep '^item|' | sed -n "$((sel + 1))p")
    IFS='|' read -r _ id title _ _ _ <<EOF
$row
EOF
}

# ------------------------------------------------------------------- run ---
scan
screen=main sel=0 top=0
# The email comes first; everything after it can show the licenses.
[ "$demo" != first ] || [ -n "$email" ] || signin
# Linux: the GPU notice comes before anything else, then the packages.
if [ "$plat" = linux ] && [ "$demo" = first ]; then
    gpu= pkgs=
    setup_gpu || exit 0
fi
if [ "$demo" = first ]; then
    draw
    choose "Install into $(short "$root")?" install install 'other folder' quit || exit 0
    case "$chosen" in
        quit) exit 0 ;;
        'other folder')
            edit "Folder:"
            case "$edited" in '') ;; '~') root=$HOME ;; '~/'*) root=$HOME/${edited#'~/'} ;; *) root=$edited ;; esac ;;
    esac
fi
if [ "$demo" = first ]; then accept_terms || exit 0; else accepted=yes; fi
if [ "$plat" = linux ] && [ "$pkgs" != ok ]; then setup_packages || exit 0; fi
if [ "$plat" = mac ] && [ "$xcode" != ok ]; then setup_xcode || :; fi
refresh
[ "$(get scope)" != partial ] || message="${warn}An earlier run stopped while downloading Makepad Scope. Select it to resume.$r0"
draw
[ "$rust" != none ] || { setup_rust; draw; }
# Start on the first licensed app.
# Start on the first licensed app, or on logging in.
if [ -z "$email" ] || [ -n "$licenses" ]; then sel=$(main_rows | sed '/^head|YOUR LICENSES/q' | grep -c '^item|'); fi

while :; do
    draw
    key
    message=
    case "$key" in
        up) [ "$sel" -eq 0 ] || sel=$((sel - 1)) ;;
        down) [ "$sel" -ge $((items - 1)) ] || sel=$((sel + 1)) ;;
        q) exit 0 ;;
        esc) if [ "$screen" = main ]; then exit 0; else screen=main sel=$back top=0; fi ;;
        enter)
            select_row
            case "$screen/$id" in
                main/account) switch_account ;;
                main/login) switch_account ;;
                main/rust) setup_rust ;;
                main/tools) if [ "$plat" = linux ]; then if [ "$pkgs" = ok ]; then spin 'Checking packages' 0.5; message="${ok}✓$r0 All development packages are installed."; else setup_packages || :; fi; elif [ "$xcode" != ok ]; then setup_xcode || :; else spin 'Checking Xcode tools' 0.5; message="${ok}✓$r0 clang, macOS SDK, linker and git are ready."; fi ;;
                main/gpu) setup_gpu || : ;;
                main/updates) check_updates ;;
                main/disk) clear_build ;;
                main/terms) terms_loop ;;
                main/free) back=$sel screen=free sel=0 top=0 ;;
                main/agent-*) open_agent "$title" ;;
                main/*)
                    if [ "$(get "$id")" = merge ]; then merge_changes "$id" "$title"; continue; fi
                    mb=$(printf '%s\n' "$licenses" | awk -F'|' -v id="$id" '$1 == id { print $4 }')
                    open_app "$id" "$title" "$mb" 214 ;;
                free/*) open_free "$id" "$title" ;;
            esac ;;
    esac
done
