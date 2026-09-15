#!/usr/bin/env bash
printf 'Makepad Arch USB\n'
cat /var/lib/makepad-provision/status 2>/dev/null || echo 'Waiting for initial setup'
printf '\nAddresses:\n'
ip -br address show scope global
printf '\nKernel: '
uname -r
printf '\nWindows mount:\n'
findmnt /mnt/win || true
printf '\nNVIDIA:\n'
if command -v nvidia-smi >/dev/null; then nvidia-smi; else echo 'NVIDIA tools not installed yet'; fi
printf '\nServices:\n'
printf 'makepad-wm.service active=%s enabled=%s\n' "$(systemctl is-active makepad-wm.service || true)" "$(systemctl is-enabled makepad-wm.service || true)"
printf 'makepad-aihub.service active=%s enabled=%s\n' "$(systemctl is-active makepad-aihub.service || true)" "$(systemctl is-enabled makepad-aihub.service || true)"
printf 'iwd.service active=%s enabled=%s\n' "$(systemctl is-active iwd.service || true)" "$(systemctl is-enabled iwd.service || true)"
printf 'Wi-Fi: use iwctl after package setup completes.\n'
printf '\nSetup log: sudo journalctl -u makepad-provision -f\n'
