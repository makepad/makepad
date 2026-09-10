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
printf '\nSetup log: sudo journalctl -u makepad-provision -f\n'
