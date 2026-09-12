#!/usr/bin/env bash
set -Eeuo pipefail
state=/var/lib/makepad-provision
seed=/run/makepad-seed
mkdir -p "$state" "$seed"
exec > >(tee -a /var/log/makepad-provision.log) 2>&1
trap 'rc=$?; printf "FAILED at line %s (exit %s); see /var/log/makepad-provision.log\n" "$LINENO" "$rc" > "$state/status"; exit "$rc"' ERR
test ! -f "$state/complete" || exit 0
stage() { printf '%s\n' "$*" | tee "$state/status"; }
stage 'Preparing persistent root filesystem'
if ! mountpoint -q "$seed"; then
    device=/dev/disk/by-label/CIDATA
    test -e "$device" || device=/dev/disk/by-label/cidata
    mount -o ro "$device" "$seed"
fi
btrfs filesystem resize max /
test "$(df --output=avail -B1 / | tail -1)" -gt 25000000000
stage 'Verifying USB payload checksums'
(cd "$seed" && sha256sum --quiet -c sha256sums)
if test ! -f "$state/mirror-extracted"; then
    stage 'Unpacking the offline Arch package mirror'
    mkdir -p /var/cache/makepad-mirror
    cat "$seed"/mirror-*.tar | tar --no-same-owner -xf - -C /var/cache/makepad-mirror
    touch "$state/mirror-extracted"
fi

stage 'Initializing package signing keys'
pacman-key --init
pacman-key --populate archlinux
stage 'Installing signed Arch packages from the USB cache'
# A temporary config leaves the normal pacman mirror configuration intact.
cat > /run/makepad-pacman.conf <<'EOF'
[options]
Architecture = x86_64
SigLevel = Required DatabaseOptional
LocalFileSigLevel = Required
ParallelDownloads = 8
CacheDir = /var/cache/pacman/pkg
CacheDir = /var/cache/makepad-mirror/core/os/x86_64
CacheDir = /var/cache/makepad-mirror/extra/os/x86_64

[core]
Server = file:///var/cache/makepad-mirror/core/os/x86_64

[extra]
Server = file:///var/cache/makepad-mirror/extra/os/x86_64
EOF
mapfile -t packages < "$seed/requested-packages.txt"
# Match the JACK provider used when resolving the offline package cache.
# Also handles seeds assembled before this provider was an explicit target.
if [[ " ${packages[*]} " != *" pipewire-jack "* ]]; then
    packages+=(pipewire-jack)
fi
pacman --config /run/makepad-pacman.conf -U --needed --noconfirm /var/cache/makepad-mirror/core/os/x86_64/archlinux-keyring-*.pkg.tar.zst
# Full upgrade against one consistent repository snapshot; never a partial -Sy.
# Adopt only the Ethernet firmware copied by firstboot before pacman was ready.
pacman --config /run/makepad-pacman.conf -Syu --needed --noconfirm --overwrite 'usr/lib/firmware/rtl_nic/*' "${packages[@]}"

stage 'Configuring graphics, CUDA, sound, and boot'
usermod -aG wheel,audio,video,input,render,storage,uucp,games arch
visudo -cf /etc/sudoers.d/10-arch
cat > /etc/modprobe.d/makepad-nvidia.conf <<'EOF'
options nvidia_drm modeset=1 fbdev=1
EOF
cat > /etc/modprobe.d/makepad-no-nouveau.conf <<'EOF'
blacklist nouveau
options nouveau modeset=0
EOF
mkdir -p /etc/mkinitcpio.conf.d
cat > /etc/mkinitcpio.conf.d/makepad.conf <<'EOF'
MODULES+=(nvidia nvidia_modeset nvidia_uvm nvidia_drm r8169 xhci_pci usbhid)
HOOKS=(base systemd microcode modconf kms keyboard sd-vconsole block filesystems fsck)
EOF
cat > /etc/profile.d/makepad-cuda.sh <<'EOF'
export CUDA_HOME=/opt/cuda
export CUDA_PATH=/opt/cuda
export NVCC_CCBIN=/usr/bin/g++-15
export CUDAHOSTCXX=/usr/bin/g++-15
case :$PATH: in *:/opt/cuda/bin:*) ;; *) export PATH=/opt/cuda/bin:$PATH ;; esac
EOF
# PAM also supplies these to non-login SSH commands used by build automation.
cat >> /etc/environment <<'EOF'
CUDA_HOME=/opt/cuda
CUDA_PATH=/opt/cuda
NVCC_CCBIN=/usr/bin/g++-15
CUDAHOSTCXX=/usr/bin/g++-15
EOF
printf '/opt/cuda/lib64\n/opt/cuda/targets/x86_64-linux/lib\n' > /etc/ld.so.conf.d/makepad-cuda.conf
ldconfig
sed -i '/^GRUB_CMDLINE_LINUX_DEFAULT=/c\GRUB_CMDLINE_LINUX_DEFAULT="rootflags=compress=zstd:1 nvidia_drm.modeset=1 nvidia_drm.fbdev=1 console=tty1 loglevel=3 module_blacklist=bluetooth,btusb"' /etc/default/grub
sed -i '/^GRUB_TERMINAL=/c\GRUB_TERMINAL="console"' /etc/default/grub
sed -i '/^GRUB_TIMEOUT=/c\GRUB_TIMEOUT=0' /etc/default/grub
if grep -q '^GRUB_TIMEOUT_STYLE=' /etc/default/grub; then
    sed -i '/^GRUB_TIMEOUT_STYLE=/c\GRUB_TIMEOUT_STYLE=hidden' /etc/default/grub
else
    printf 'GRUB_TIMEOUT_STYLE=hidden\n' >> /etc/default/grub
fi
mkinitcpio -P
grub-mkconfig -o /boot/grub/grub.cfg
systemctl set-default multi-user.target
systemctl enable sshd systemd-resolved systemd-timesyncd
systemctl mask systemd-networkd-wait-online.service systemd-time-wait-sync.service pacman-init.service bluetooth.service wpa_supplicant.service
systemctl --global enable pipewire.socket pipewire-pulse.socket wireplumber.service
loginctl enable-linger arch
systemctl enable --now makepad-mount-win.service
systemctl unmask iwd.service
systemctl enable --now iwd.service
networkctl reload

stage 'Installing pinned Fanatec driver source and DKMS module'
fanatec_version=0.0.dd78ef477c0d
fanatec_source=/usr/src/hid-fanatec-$fanatec_version
mkdir -p "$fanatec_source"
tar -xzf "$seed/fanatec.tgz" --strip-components=1 -C "$fanatec_source"
find "$fanatec_source" -type f \( -name dkms.conf -o -name '*.c' \) -exec sed -i "s/#VERSION#/$fanatec_version/g" {} +
install -m 0644 "$fanatec_source/fanatec.rules" /etc/udev/rules.d/99-fanatec.rules
if ! dkms status -m hid-fanatec -v "$fanatec_version" | grep -q .; then
    dkms add -m hid-fanatec -v "$fanatec_version"
fi
fanatec_ok=yes
for directory in /usr/lib/modules/*; do
    test -d "$directory/build" || continue
    kernel=${directory##*/}
    if ! dkms install -m hid-fanatec -v "$fanatec_version" -k "$kernel"; then
        fanatec_ok=no
    fi
done
printf '%s\n' "$fanatec_ok" > "$state/fanatec-built"
install -o root -g root -m 0755 "$seed/gbelt-bind.sh" /usr/local/sbin/makepad-gbelt-bind
install -o root -g root -m 0644 "$seed/70-makepad-game-hardware.rules" /etc/udev/rules.d/70-makepad-game-hardware.rules
udevadm control --reload-rules

stage 'Unpacking the Makepad source snapshot and Cargo cache'
mkdir -p /home/arch/makepad /home/arch/.cargo
if test ! -f "$state/source-installed"; then
    tar -xzf "$seed/makepad-source.tgz" -C /home/arch/makepad
    tar -xf "$seed/cargo-cache.tar" -C /home/arch/.cargo
    touch "$state/source-installed"
fi
stage 'Installing the offline Linux CEF distribution'
cef_root=/home/arch/makepad/local/cef-prebuilt
cef_directory=$(cat "$seed/cef-directory.txt")
[[ "$cef_directory" = cef_binary_*_linux64 && "$cef_directory" != */* ]]
mkdir -p "$cef_root"
if test ! -f "$state/cef-installed"; then
    tar --no-same-owner -xjf "$seed/cef-linux.bz2" -C "$cef_root"
    test -f "$cef_root/$cef_directory/Release/libcef.so"
    ln -sfn "$cef_directory" "$cef_root/current-linux64"
    touch "$state/cef-installed"
fi
chown -R arch:arch /home/arch/makepad /home/arch/.cargo
install -o arch -g arch -m 0644 "$seed/source-revision.txt" /home/arch/makepad/SOURCE-REVISION.txt
runuser -u arch -- git -C /home/arch/makepad init
runuser -u arch -- git -C /home/arch/makepad lfs install --local

stage 'Installing the direct Vulkan WM launcher'
install -d /etc/makepad
install -m 0755 "$seed/wm-session.sh" /usr/local/bin/makepad-wm-session
install -m 0644 "$seed/wm.env" /etc/makepad/wm.env
install -m 0644 "$seed/makepad-wm.service" /etc/systemd/system/makepad-wm.service
install -m 0755 "$seed/aihub-session.sh" /usr/local/bin/makepad-aihub-session
install -m 0644 "$seed/aihub.env" /etc/makepad/aihub.env
install -m 0644 "$seed/makepad-aihub.service" /etc/systemd/system/makepad-aihub.service
systemctl daemon-reload
systemctl enable makepad-wm.service makepad-aihub.service

stage 'Recording installed tool versions' 
source /etc/profile.d/makepad-cuda.sh
{
    rustc --version
    cargo --version
    nvcc --version
    pacman -Q linux nvidia-open nvidia-utils cuda cudnn
    lspci -nnk
    lsusb
    dkms status
} > "$state/installed.txt" 2>&1
stage 'Building release WM and CUDA AI Hub offline'
runuser -u arch -- env HOME=/home/arch MAKEPAD=linux_direct+vulkan CARGO_NET_OFFLINE=true MAKEPAD_CEF_OFFLINE=1 CUDA_HOME=/opt/cuda CUDA_PATH=/opt/cuda NVCC_CCBIN=/usr/bin/g++-15 CUDAHOSTCXX=/usr/bin/g++-15 PATH="$PATH" bash -c 'cd /home/arch/makepad && cargo build --offline --release -p makepad-wm -p makepad-app-ai-hub'
sshd -t
systemctl reload sshd
touch "$state/complete"
touch /etc/cloud/cloud-init.disabled
if test "$fanatec_ok" = yes; then
    stage 'Package setup complete. Reboot required for the installed kernel and NVIDIA driver.'
else
    stage 'Package setup complete; Fanatec DKMS needs inspection. Reboot required for the installed kernel/NVIDIA driver.'
fi
# Keep SSH available for inspection. The operator can reboot when ready.
