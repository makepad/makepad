#!/bin/sh
# Check readiness before invoking macOS Git shims that might implicitly
# open an installer. Never accept Apple's license on the user's behalf.
set -eu

apple_status() {
    selected=$(/usr/bin/xcode-select -p 2>/dev/null || :)
    apple_ok=yes
    printf '\n  Apple developer tools\n\n'
    if [ -d /Applications/Xcode.app ]; then
        printf '  Xcode app          Installed\n'
    elif [ -n "$selected" ] && [ -x "$selected/usr/bin/xcodebuild" ]; then
        printf '  Xcode app          Installed at the selected location\n'
    else
        printf '  Xcode app          Not installed (standalone tools are sufficient)\n'
    fi
    if [ -z "$selected" ] || [ ! -d "$selected" ]; then
        printf '  Active tools       Missing\n'
        apple_ok=no
        return
    fi
    printf '  Active tools       %s\n' "$selected"
    if [ -x "$selected/usr/bin/xcodebuild" ]; then
        if /usr/bin/xcodebuild -checkFirstLaunchStatus >/dev/null 2>&1; then
            printf '  License / setup    Ready\n'
        else
            printf '  License / setup    Needs attention — finish Xcode first launch\n'
            apple_ok=no
            return
        fi
    else
        printf '  License / setup    Standalone tools; checking they are usable\n'
    fi
    if /usr/bin/xcrun --sdk macosx clang --version >/dev/null 2>&1 && /usr/bin/xcrun --sdk macosx --show-sdk-path >/dev/null 2>&1; then
        printf '  Compiler + SDK     Ready\n'
    else
        printf '  Compiler + SDK     Missing or awaiting Apple setup / acceptance\n'
        apple_ok=no
    fi
    if command -v git >/dev/null 2>&1 && git --version >/dev/null 2>&1; then
        printf '  Git                Ready\n'
    else
        printf '  Git                Unavailable — included with Apple developer tools\n'
        apple_ok=no
    fi
}

case "$(uname -s)" in
    Darwin)
        while :; do
            apple_status
            [ "$apple_ok" = yes ] && break
            [ "${1:-}" = --check ] && exit 1
            printf '\n  Complete Apple’s installation and accept its terms in Apple’s UI.\n'
            printf '  If tools are installed elsewhere, select them with:\n'
            printf '    sudo xcode-select --switch /path/to/Xcode.app/Contents/Developer\n'
            printf '\n  i  Open Command Line Tools installation (xcode-select --install)\n'
            printf '  o  Open the selected Xcode app to finish setup\n'
            printf '  l  Read and accept the Xcode license interactively (sudo)\n'
            printf '  r  Recheck    q  Quit\n\n  Choose: '
            IFS= read -r answer
            case "$answer" in
                i|I) /usr/bin/xcode-select --install || : ;;
                o|O)
                    case "$selected" in
                        *.app/Contents/Developer) /usr/bin/open "${selected%/Contents/Developer}" ;;
                        *) if [ -d /Applications/Xcode.app ]; then /usr/bin/open /Applications/Xcode.app; else printf '\n  Install Xcode from Apple, or use standalone Command Line Tools.\n'; fi ;;
                    esac ;;
                l|L)
                    if [ -n "$selected" ] && [ -x "$selected/usr/bin/xcodebuild" ]; then
                        sudo /usr/bin/xcodebuild -license || :
                    else
                        printf '\n  Standalone tools have their license in Apple’s installation dialog.\n'
                    fi ;;
                q|Q) exit 1 ;;
            esac
        done
        ;;
    Linux)
        if ! getconf GNU_LIBC_VERSION >/dev/null 2>&1; then
            printf '%s\n' 'This Builder release requires glibc Linux (x86_64 or ARM64).'
            exit 1
        fi
        linux_ready() {
            for tool in cc c++ make cmake git curl tar gzip pkg-config; do
                command -v "$tool" >/dev/null 2>&1 || return 1
            done
            pkg-config --exists openssl x11 xcursor xkbcommon xrandr xi xinerama alsa libpulse wayland-client egl gl glx libdrm gbm
        }
        if ! linux_ready; then
            [ "${1:-}" = --check ] && exit 1
            ID= ID_LIKE=
            if [ -r /etc/os-release ]; then . /etc/os-release; fi
            case " $ID $ID_LIKE " in
                *debian*|*ubuntu*) set -- apt-get install --no-install-recommends build-essential clang cmake pkg-config git curl ca-certificates tar gzip libssl-dev libx11-dev libxcursor-dev libxkbcommon-dev libxrandr-dev libxi-dev libxinerama-dev libasound2-dev libpulse-dev libwayland-dev wayland-protocols libegl-dev libgl-dev libglx-dev libdrm-dev libgbm-dev ;;
                *fedora*|*rhel*|*centos*) set -- dnf install gcc gcc-c++ make clang cmake pkgconf-pkg-config git curl ca-certificates tar gzip openssl-devel libX11-devel libXcursor-devel libxkbcommon-devel libXrandr-devel libXi-devel libXinerama-devel alsa-lib-devel pulseaudio-libs-devel wayland-devel wayland-protocols-devel libglvnd-devel libdrm-devel mesa-libgbm-devel ;;
                *arch*) set -- pacman -S --needed base-devel clang cmake pkgconf git curl ca-certificates tar gzip openssl libx11 libxcursor libxkbcommon libxrandr libxi libxinerama alsa-lib libpulse wayland wayland-protocols libglvnd mesa libdrm ;;
                *suse*) set -- zypper install gcc gcc-c++ make clang cmake pkg-config git curl ca-certificates tar gzip libopenssl-devel libX11-devel libXcursor-devel libxkbcommon-devel libXrandr-devel libXi-devel libXinerama-devel alsa-devel libpulse-devel wayland-devel wayland-protocols-devel libglvnd-devel libdrm-devel Mesa-libgbm-devel ;;
                *) printf '%s\n' 'Install C/C++, Git, curl, CMake, pkg-config, OpenSSL, X11, Xcursor, xkbcommon, ALSA, PulseAudio, Wayland, EGL/GLX and GBM development packages with your distro package manager, then retry.'; exit 1 ;;
            esac
            printf '\n  System development packages for %s\n' "${PRETTY_NAME:-Linux}"
            printf '  These libraries are installed globally by your package manager.\n'
            printf '  Rust and Cargo will remain private to the Builder folder.\n\n'
            printf '  sudo %s\n\n  Continue? [Y/n] ' "$*"
            IFS= read -r answer
            case "$answer" in ''|y|Y|yes|Yes) sudo "$@" ;; *) exit 1 ;; esac
            if ! linux_ready; then
                printf '%s\n' 'Some development libraries are still unavailable; check the package-manager output above.'
                exit 1
            fi
        fi
        ;;
    *) printf '%s\n' 'Makepad Builder supports macOS and Linux.'; exit 1 ;;
esac
if ! command -v curl >/dev/null 2>&1; then
    printf '%s\n' 'curl is required for verified HTTPS downloads.'
    exit 1
fi
