#!/bin/bash
# Copyright(c) 2025 Meta Platforms, Inc. and affiliates.
#
# This source code is subject to the terms of the BSD 2 Clause License and
# the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
# was not distributed with this source code in the LICENSE file, you can
# obtain it at https://www.aomedia.org/license/software-license. If the
# Alliance for Open Media Patent License 1.0 was not distributed with this
# source code in the PATENTS file, you can obtain it at
# https://www.aomedia.org/license/patent-license.

## This scripts installs the dependencies for image and video coding evaluation.
## 1. installs the required libraries (yasm, nasm),
## 2. sets up a conda environment and installs required packages (pyyaml and meson)
## 3. clones and builds base codecs.
##
## The conda env is called codec_eval and can be activated in the future runs from the same machine:
## conda activate codec_eval

# perform platform specific setup
OS_TYPE="$(uname)"
if [[ $OS_TYPE == "Darwin" ]]; then
    echo "Detected macOS..."

    # install required build libraries
    if command -v brew > /dev/null 2>&1; then
        echo "Homebrew is already installed, skipping"
    else
        /bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"
    fi
    brew install nasm
    brew install yasm
    brew install meson
    brew install cmake
    brew install ffmpeg

    if ! command -v conda > /dev/null 2>&1; then
        # Install conda for MacOS
        curl -O https://repo.anaconda.com/miniconda/Miniconda3-latest-MacOSX-arm64.sh
        zsh Miniconda3-latest-MacOSX-arm64.sh -b -p ~/miniconda

        # Add conda to PATH for current session
        export PATH="$PATH:$HOME/miniconda/bin"

        # Initialize conda for future sessions
        ~/miniconda/bin/conda init zsh
        echo "Conda installed. You may need to restart your shell or run 'source ~/.zshrc'"
    else
        echo "Conda is already installed, skipping"
    fi

elif [[ $OS_TYPE == "Linux" ]]; then
    echo "Detected Linux..."

    # Detect Linux distribution
    if [[ -f /etc/os-release ]]; then
        . /etc/os-release
        DISTRO=$ID
    elif [[ -f /etc/redhat-release ]]; then
        DISTRO="rhel"
    elif [[ -f /etc/debian_version ]]; then
        DISTRO="debian"
    else
        echo "Warning: Cannot detect Linux distribution, assuming Ubuntu/Debian"
        DISTRO="ubuntu"
    fi

    echo "Detected Linux distribution: $DISTRO"

    # Install required build libraries based on distribution
    case $DISTRO in
    ubuntu | debian | pop)
        echo "Installing packages with apt..."
        sudo apt-get update
        sudo apt-get install -y nasm yasm meson cmake build-essential pkg-config
        # Try to install ffmpeg, but don't fail if not available
        sudo apt-get install -y ffmpeg || echo "Warning: ffmpeg not available, will skip"
        ;;
    fedora | centos | rhel | rocky | alma)
        echo "Installing packages with dnf/yum..."
        if command -v dnf > /dev/null 2>&1; then
            PKG_MGR="dnf"
        else
            PKG_MGR="yum"
        fi
        sudo $PKG_MGR install -y nasm yasm meson cmake gcc gcc-c++ make pkgconfig
        sudo $PKG_MGR install -y ffmpeg-devel || sudo $PKG_MGR install -y ffmpeg || echo "Warning: ffmpeg not available, will skip"
        ;;
    arch | manjaro)
        echo "Installing packages with pacman..."
        sudo pacman -Sy --noconfirm nasm yasm meson cmake base-devel pkgconf
        sudo pacman -S --noconfirm ffmpeg || echo "Warning: ffmpeg not available, will skip"
        ;;
    opensuse* | sles)
        echo "Installing packages with zypper..."
        sudo zypper install -y nasm yasm meson cmake gcc gcc-c++ make pkg-config
        sudo zypper install -y ffmpeg-4-libavcodec-devel || sudo zypper install -y ffmpeg || echo "Warning: ffmpeg not available, will skip"
        ;;
    *)
        echo "Unsupported Linux distribution: $DISTRO"
        echo "Please install the following packages manually:"
        echo "  - nasm, yasm, meson, cmake, build tools, pkg-config"
        echo "  - ffmpeg (optional but recommended)"
        echo "Continuing with conda installation..."
        ;;
    esac

    # Install conda for Linux
    if ! command -v conda > /dev/null 2>&1; then
        echo "Installing Miniconda..."
        ARCH=$(uname -m)
        if [[ $ARCH == "x86_64" ]]; then
            CONDA_ARCH="x86_64"
        elif [[ $ARCH == "aarch64" ]] || [[ $ARCH == "arm64" ]]; then
            CONDA_ARCH="aarch64"
        else
            echo "Warning: Unsupported architecture $ARCH, assuming x86_64"
            CONDA_ARCH="x86_64"
        fi

        curl -O "https://repo.anaconda.com/miniconda/Miniconda3-latest-Linux-${CONDA_ARCH}.sh"
        bash "Miniconda3-latest-Linux-${CONDA_ARCH}.sh" -b -p ~/miniconda
        rm "Miniconda3-latest-Linux-${CONDA_ARCH}.sh"

        # Add conda to PATH for current session
        export PATH="$HOME/miniconda/bin:$PATH"

        # Initialize conda for future sessions
        ~/miniconda/bin/conda init bash
        echo "Conda installed. You may need to restart your shell or run 'source ~/.bashrc'"
    else
        echo "Conda is already installed, skipping"
    fi

else
    echo "Unsupported OS: $OS_TYPE"
    exit 1
fi

# now do common steps

conda tos accept --override-channels --channel https://repo.anaconda.com/pkgs/main
conda tos accept --override-channels --channel https://repo.anaconda.com/pkgs/r

# create a conda environment and install required packages
conda create -y -n codec_eval python=3.12
conda init
conda activate codec_eval
pip install -r requirements.txt

DEF_DIR=~/benchmark
DST_DIR=${1:-${DEF_DIR}}

# create directory structure
mkdir -p "${DST_DIR}"/bin/mac_arm64
mkdir -p "${DST_DIR}"/bin/linux_x64
mkdir -p "${DST_DIR}"/configs
mkdir -p "${DST_DIR}"/DataSet

CUR_DIR="$PWD"

###############################################################################
#
# clone and build base codecs
#
###############################################################################

# Copy scripts
rsync -a scripts "${DST_DIR}"/bin

if [[ $OS_TYPE == "Darwin" ]]; then
    BIN_DIR="${DST_DIR}"/bin/mac_arm64
elif [[ $OS_TYPE == "Linux" ]]; then
    BIN_DIR="${DST_DIR}"/bin/linux_x64
else
    echo "Unsupported OS: $OS_TYPE"
    exit 1
fi

# Iterate over each subdirectory
for dir in "${DST_DIR}/bin/scripts"/*/; do
    if [ -d "$dir" ]; then
        echo "Getting: $dir"
        cd "$dir" || exit 1
        ./get_and_build.sh "${BIN_DIR}"
    fi
done

cd "$CUR_DIR" || exit 1

###############################################################################
#
# get base datasets from AOM CTC
# https://media.xiph.org/video/aomctc/test_set
#
###############################################################################

# Images
TARGET_DIR="${DST_DIR}/DataSet/f2_still_MidRes"
mkdir -p "$TARGET_DIR"
cd "$TARGET_DIR" || exit 1

for i in {1..3}; do
    BASE_URL="https://media.xiph.org/video/aomctc/test_set/f2_still_MidRes/part${i}/"

    curl -s "${BASE_URL}" |
        sed -n 's/.*href="\([^"]*\.y4m\)".*/\1/p' |
        while read -r file; do
            curl -O "${BASE_URL}${file}"
        done
done

# Video
TARGET_DIR="${DST_DIR}/DataSet/a4_360p"
mkdir -p "$TARGET_DIR"
cd "$TARGET_DIR" || exit 1

BASE_URL="https://media.xiph.org/video/aomctc/test_set/a4_360p/"

curl -s "${BASE_URL}" |
    sed -n 's/.*href="\([^"]*\.y4m\)".*/\1/p' |
    while read -r file; do
        curl -O "${BASE_URL}${file}"
    done
