#!/usr/bin/env bash
# Only read NTFS volumes. Never clear hibernation, repair, or force a dirty mount.
set -euo pipefail
mkdir -p /mnt/win /run/makepad-ntfs-probe
mountpoint -q /mnt/win && exit 0
if findmnt --fstab --mountpoint /mnt/win >/dev/null; then
    mount /mnt/win
    exit 0
fi
root_disk=$(lsblk -ndo PKNAME "$(findmnt -nro SOURCE / | sed 's/\[.*//')")
best_uuid=
best_size=0
cleanup() { umount /run/makepad-ntfs-probe 2>/dev/null || true; }
trap cleanup EXIT
while IFS= read -r device; do
    parent=$(lsblk -ndo PKNAME "$device")
    test "$parent" != "$root_disk" || continue
    if mount -t ntfs3 -o ro "$device" /run/makepad-ntfs-probe; then
        if test -d /run/makepad-ntfs-probe/Windows/System32 && test -d /run/makepad-ntfs-probe/Users; then
            size=$(blockdev --getsize64 "$device")
            if (( size > best_size )); then
                best_uuid=$(blkid -s UUID -o value "$device")
                best_size=$size
            fi
        fi
        umount /run/makepad-ntfs-probe
    fi
done < <(blkid -t TYPE=ntfs -o device || true)
if test -z "$best_uuid"; then
    echo 'No readable Windows NTFS OS volume found; /mnt/win is empty.'
    exit 0
fi
printf 'UUID=%s /mnt/win ntfs3 ro,nofail,uid=1000,gid=1000,umask=022,x-systemd.device-timeout=5s 0 0\n' "$best_uuid" >> /etc/fstab
systemctl daemon-reload
mount /mnt/win
