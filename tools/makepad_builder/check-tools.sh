#!/bin/sh
# Checks the system tools the Builder compiles with.
#
#   check-tools.sh           check, and offer to install what is missing
#   check-tools.sh --check   check only: exit 0 when ready, 1 when not
#
# macOS: Apple's command line developer tools (clang, the macOS SDK, the
#   linker and git). Xcode itself is not needed. The only fix offered is
#   Apple's own installer; an Xcode license is never accepted for you.
# Linux: C/C++, git, curl and the development libraries Makepad apps use,
#   installed with your distribution's package manager after you see the
#   exact command. This is the one step that changes your system.
set -eu

check_only=no
[ "${1:-}" != --check ] || check_only=yes

say()  { printf '  %s\n' "$*"; }
fail() { printf '\n  %s\n\n' "$*" >&2; exit 1; }

# --- macOS ------------------------------------------------------------------

apple_problem() {
    # Echoes nothing when ready, otherwise one line saying what is wrong.
    # Probing through xcrun reports an unaccepted Xcode license as an error
    # instead of opening a prompt.
    developer=${DEVELOPER_DIR:-$(/usr/bin/xcode-select -p 2>/dev/null || :)}
    if [ -z "$developer" ] || [ ! -d "$developer" ]; then
        echo missing; return
    fi
    for probe in 'clang --version' '--show-sdk-path' '--find ld'; do
        # shellcheck disable=SC2086 # the probe words are fixed above
        if ! detail=$(/usr/bin/xcrun --sdk macosx $probe 2>&1); then
            case "$detail" in *license*) echo license ;; *) echo missing ;; esac
            return
        fi
    done
    git --version >/dev/null 2>&1 || { echo missing; return; }
}

check_apple() {
    problem=$(apple_problem)
    [ -n "$problem" ] || { say "Apple's command line developer tools are ready."; return 0; }
    [ "$check_only" = no ] || exit 1
    case "$problem" in
        missing)
            say "Apple's command line developer tools are not installed (Xcode itself is not needed)."
            say "Opening Apple's installer. When it has finished, run the Builder again."
            /usr/bin/xcode-select --install >/dev/null 2>&1 || :
            ;;
        license)
            say 'Xcode is installed, but its license has not been accepted yet.'
            say 'Read and accept it with this command, then run the Builder again:'
            say '    sudo xcodebuild -license'
            ;;
    esac
    exit 1
}

# --- Linux ------------------------------------------------------------------

linux_ready() {
    for tool in cc c++ make cmake git curl tar gzip pkg-config; do
        command -v "$tool" >/dev/null 2>&1 || return 1
    done
    pkg-config --exists openssl x11 xcursor xkbcommon xrandr xi xinerama alsa libpulse \
        wayland-client egl gl glx libdrm gbm
}

check_linux() {
    getconf GNU_LIBC_VERSION >/dev/null 2>&1 || fail 'The Builder needs glibc Linux (x86_64 or ARM64).'
    if linux_ready; then say 'System development packages are ready.'; return 0; fi
    [ "$check_only" = no ] || exit 1

    ID= ID_LIKE= PRETTY_NAME=
    [ ! -r /etc/os-release ] || . /etc/os-release
    case " $ID $ID_LIKE " in
        *debian* | *ubuntu*)
            set -- apt-get install --no-install-recommends build-essential clang cmake pkg-config \
                git curl ca-certificates tar gzip libssl-dev libx11-dev libxcursor-dev libxkbcommon-dev \
                libxrandr-dev libxi-dev libxinerama-dev libasound2-dev libpulse-dev libwayland-dev \
                wayland-protocols libegl-dev libgl-dev libglx-dev libdrm-dev libgbm-dev ;;
        *fedora* | *rhel* | *centos*)
            set -- dnf install gcc gcc-c++ make clang cmake pkgconf-pkg-config git curl \
                ca-certificates tar gzip openssl-devel libX11-devel libXcursor-devel libxkbcommon-devel \
                libXrandr-devel libXi-devel libXinerama-devel alsa-lib-devel pulseaudio-libs-devel \
                wayland-devel wayland-protocols-devel libglvnd-devel libdrm-devel mesa-libgbm-devel ;;
        *arch*)
            set -- pacman -S --needed base-devel clang cmake pkgconf git curl ca-certificates tar \
                gzip openssl libx11 libxcursor libxkbcommon libxrandr libxi libxinerama alsa-lib \
                libpulse wayland wayland-protocols libglvnd mesa libdrm ;;
        *suse*)
            set -- zypper install gcc gcc-c++ make clang cmake pkg-config git curl ca-certificates \
                tar gzip libopenssl-devel libX11-devel libXcursor-devel libxkbcommon-devel \
                libXrandr-devel libXi-devel libXinerama-devel alsa-devel libpulse-devel wayland-devel \
                wayland-protocols-devel libglvnd-devel libdrm-devel Mesa-libgbm-devel ;;
        *)
            fail 'Install C/C++, git, curl, CMake, pkg-config and the OpenSSL, X11, Xcursor, xkbcommon, ALSA, PulseAudio, Wayland, EGL/GLX and GBM development packages with your package manager, then run the Builder again.' ;;
    esac

    say "Makepad compiles from source and needs development packages from ${PRETTY_NAME:-your distribution}."
    say 'They are installed system-wide with this command:'
    printf '\n    sudo %s\n\n' "$*"
    printf '  Press Return to run it (sudo asks for your password), or Ctrl+C to stop. '
    IFS= read -r answer || fail 'Stopped. Nothing was installed.'
    sudo "$@"
    linux_ready || fail 'Some development packages are still missing; see the package manager output above.'
    say 'System development packages are ready.'
}

# --- Run --------------------------------------------------------------------

case "$(uname -s)" in
    Darwin) check_apple ;;
    Linux)  check_linux ;;
    *)      fail 'The Builder supports macOS and Linux.' ;;
esac
command -v curl >/dev/null 2>&1 || fail 'curl is needed for verified downloads.'
