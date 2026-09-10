#!/usr/bin/bash
# Configure the fresh official Arch root. Run directly from the read-only seed.
set -Eeuo pipefail
export PATH=/usr/local/sbin:/usr/local/bin:/usr/bin
seed=/run/makepad-seed
state=/var/lib/makepad-firstboot
mkdir -p "$state" /var/log
exec > >(tee -a /var/log/makepad-firstboot.log) 2>&1
trap 'rc=$?; echo "First-boot setup failed at line $LINENO (exit $rc). See /var/log/makepad-firstboot.log"; exit "$rc"' ERR
test ! -f "$state/complete" || exit 0
echo 'Configuring console login, sudo, SSH and Ethernet.'
(cd "$seed" && sha256sum --quiet -c config.sha256)
test -x /usr/bin/pacman

for group in wheel audio video input render storage uucp games; do
    getent group "$group" >/dev/null || groupadd -r "$group"
done
getent group arch >/dev/null || groupadd arch
id arch >/dev/null 2>&1 || useradd -m -u 1000 -g arch -s /bin/bash arch
usermod -aG wheel,audio,video,input,render,storage,uucp,games -s /bin/bash arch
printf 'arch:12345\n' | chpasswd
chage -E -1 -I -1 -M 99999 arch
install -d -m 0700 -o arch -g arch /home/arch/.ssh
install -o arch -g arch -m 0600 "$seed/authorized_keys" /home/arch/.ssh/authorized_keys
chmod go-w /home/arch
install -d -m 0750 /etc/sudoers.d
printf 'arch ALL=(ALL:ALL) ALL\n' > /etc/sudoers.d/10-arch
chmod 0440 /etc/sudoers.d/10-arch
grep -Eq '^([@#]includedir)[[:space:]]+/etc/sudoers.d([[:space:]]|$)' /etc/sudoers || printf '\n@includedir /etc/sudoers.d\n' >> /etc/sudoers
visudo -c
sudo -l -U arch /usr/bin/id

mkdir -p /etc/ssh/sshd_config.d
install -m 0644 "$seed/sshd.conf" /etc/ssh/sshd_config.d/00-makepad.conf
if ! head -1 /etc/ssh/sshd_config | grep -Fxq 'Include /etc/ssh/sshd_config.d/00-makepad.conf'; then
    { printf 'Include /etc/ssh/sshd_config.d/00-makepad.conf\n'; cat /etc/ssh/sshd_config; } > "$state/sshd_config.new"
    install -m 0600 "$state/sshd_config.new" /etc/ssh/sshd_config
fi
ssh-keygen -A
sshd -t
sshd -T -C user=arch,host=makepad-arch,addr=127.0.0.1 > "$state/sshd-effective.txt"
for policy in 'pubkeyauthentication yes' 'passwordauthentication no' 'kbdinteractiveauthentication no' 'permitrootlogin no' 'authenticationmethods publickey' 'allowusers arch'; do
    grep -Fxiq "$policy" "$state/sshd-effective.txt"
done

# Firmware is available before re-probing the Realtek Ethernet adapter.
mkdir -p /usr/lib/firmware /etc/systemd/network /etc/modprobe.d
tar -xf "$seed/realtek-firmware.tar" -C /usr/lib/firmware
install -m 0644 "$seed/wired.network" /etc/systemd/network/05-makepad-wired.network
install -m 0644 "$seed/no-wireless.conf" /etc/modprobe.d/makepad-no-wireless.conf
ln -sfn /run/systemd/resolve/stub-resolv.conf /etc/resolv.conf
printf 'makepad-arch\n' > /etc/hostname
printf 'makepad-arch\n' > /proc/sys/kernel/hostname
printf '127.0.0.1 localhost\n::1 localhost\n127.0.1.1 makepad-arch\n' > /etc/hosts
printf 'LANG=C.UTF-8\n' > /etc/locale.conf
ln -sfn /usr/share/zoneinfo/Europe/Amsterdam /etc/localtime
mkdir -p /usr/local/sbin /usr/local/bin /etc/udev/rules.d
install -m 0755 "$seed/provision.sh" /usr/local/sbin/makepad-provision
install -m 0755 "$seed/mount-win.sh" /usr/local/sbin/makepad-mount-win
install -m 0755 "$seed/status.sh" /usr/local/bin/makepad-status
install -m 0644 "$seed/makepad-provision.service" /etc/systemd/system/makepad-provision.service
install -m 0644 "$seed/makepad-mount-win.service" /etc/systemd/system/makepad-mount-win.service
install -m 0644 "$seed/fanatec-access.rules" /etc/udev/rules.d/70-makepad-fanatec-access.rules

# Networkd is already enabled in the checked base. Enabling it again would
# also enable its wait-online service, which is deliberately masked at boot.
systemctl --root=/ unmask sshd.service systemd-resolved.service
systemctl --root=/ is-enabled systemd-networkd.service
systemctl --root=/ enable sshd.service systemd-resolved.service systemd-timesyncd.service makepad-provision.service makepad-mount-win.service
for unit in pacman-init.service systemd-networkd-wait-online.service systemd-networkd-wait-online@.service systemd-time-wait-sync.service bluetooth.service iwd.service wpa_supplicant.service; do
    systemctl --root=/ disable "$unit" || true
    if test -f "/etc/systemd/system/$unit" && ! test -L "/etc/systemd/system/$unit"; then
        mv "/etc/systemd/system/$unit" "$state/$unit.original"
    fi
    systemctl --root=/ mask --force "$unit"
done
systemctl --root=/ set-default multi-user.target
systemctl --root=/ is-enabled sshd.service
systemctl daemon-reload
modprobe r8169 || echo 'Realtek module loading needs inspection; see this log.'
if test "${MAKEPAD_KEEP_NETWORK:-0}" != 1; then
    networkctl reload || echo 'Networkd will load the Ethernet configuration when it starts.'
    for path in /sys/class/net/eth* /sys/class/net/en*; do
        test -e "$path" || continue
        networkctl reconfigure "${path##*/}" || echo "Networkd will configure ${path##*/} when the interface appears."
    done
fi

printf 'Makepad Arch USB - login: arch\nIPv4: \\4\nSSH uses the Mac key. Run makepad-status for package setup progress.\n\n' > /etc/issue
printf 'Makepad Arch USB\nInitial console/sudo password: 12345. SSH uses your Mac key.\npacman is available; initial package installation runs in the background.\nRun makepad-status for progress.\n' > /etc/motd

# SSH is ordered after this cloud-init stage. Queue it without waiting for
# that dependency; returning from this script lets it start normally.
systemctl start --no-block systemd-networkd.service systemd-resolved.service sshd.service makepad-provision.service
sync
touch "$state/complete"
sync
echo 'First-boot configuration validated. SSH is queued; package setup follows SSH startup.'
