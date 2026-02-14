#!/usr/bin/env bash
set -euo pipefail

if [[ "${EUID}" -eq 0 ]]; then
    SUDO=""
else
    SUDO="sudo"
fi

# Linux build/runtime deps for Makepad on Ubuntu/WSL2:
# - X11 backend
# - Wayland backend
# - OpenGL/EGL
# - Audio (ALSA + PulseAudio)
# - TLS for HTTP(S)/WSS (OpenSSL)
${SUDO} apt-get update
${SUDO} apt-get install -y --no-install-recommends \
    build-essential \
    pkg-config \
    clang \
    ca-certificates \
    libssl-dev \
    libx11-dev \
    libxcursor-dev \
    libxkbcommon-dev \
    libxrandr-dev \
    libxi-dev \
    libxinerama-dev \
    libasound2-dev \
    libpulse-dev \
    libwayland-dev \
    wayland-protocols \
    libegl1-mesa-dev \
    libgl1-mesa-dev \
    libgles2-mesa-dev \
    libglx-dev \
    libdrm-dev \
    libgbm-dev \
    libgl1-mesa-dri \
    mesa-vulkan-drivers \
    mesa-utils \
    mesa-utils-extra \
    x11-apps

echo "Makepad Linux dependencies installed."
