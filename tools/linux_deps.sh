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
# - GStreamer video/audio playback (HLS/DASH needs good+bad+libav)
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
    x11-apps \
    gstreamer1.0-tools \
    gstreamer1.0-plugins-base \
    gstreamer1.0-plugins-good \
    gstreamer1.0-plugins-bad \
    gstreamer1.0-plugins-ugly \
    gstreamer1.0-libav \
    gstreamer1.0-gl \
    gstreamer1.0-alsa \
    gstreamer1.0-pipewire \
    libgstreamer1.0-0 \
    libgstreamer-plugins-base1.0-0 \
    libgstreamer-gl1.0-0

echo "Makepad Linux dependencies installed."
