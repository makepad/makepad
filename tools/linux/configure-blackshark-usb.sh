#!/usr/bin/env bash
# Configure existing Linux drivers for BlackShark V3 Pro (1532:0577).
# Run as root on a GRUB-based system. No packages or drivers are installed.
# After first configuration, physically unplug the receiver for five seconds
# and reconnect it: a warm USB reset can retain the firmware's host state.
set -euo pipefail
if (( EUID != 0 )); then
    echo 'Run this configuration script as root.' >&2
    exit 1
fi
need() {
    command -v "$1" >/dev/null || { echo "This configuration needs $1, which is not installed." >&2; exit 1; }
}
need python3
need grub-mkconfig
need udevadm
# Only the GRUB configuration this system boots is regenerated; anything
# else (another boot loader, /boot/grub2) is left for manual setup.
if [[ ! -f /boot/grub/grub.cfg ]]; then
    echo 'No /boot/grub/grub.cfg: add usbcore.quirks=1532:0577:r to the kernel command line manually.' >&2
    exit 1
fi
blackshark_script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
test -f "$blackshark_script_dir/70-makepad-blackshark.rules"
python3 - <<'PY'
from pathlib import Path
import re
import shutil
import subprocess

def merge_quirk(value):
    entries = [part for part in value.strip().split(',') if part]
    matches = [part for part in entries if part.startswith('1532:0577:')]
    if len(matches) > 1:
        raise SystemExit('Duplicate BlackShark quirks; inspect the existing configuration.')
    if matches:
        previous = matches[0]
        if 'r' not in previous.split(':')[2]:
            entries[entries.index(previous)] = previous + 'r'
    else:
        entries.append('1532:0577:r')
    return ','.join(entries)

grub = Path('/etc/default/grub')
original = grub.read_text()
pattern = r'^GRUB_CMDLINE_LINUX_DEFAULT=([\x22\x27])([^\n]*?)\1[ \t]*$'
matches = list(re.finditer(pattern, original, re.M))
if len(matches) != 1:
    raise SystemExit('Expected one quoted GRUB_CMDLINE_LINUX_DEFAULT in /etc/default/grub.')
match = matches[0]
words = match[2].split()
quirks = [word for word in words if word.startswith('usbcore.quirks=')]
if len(quirks) > 1 or re.search(r'^GRUB_CMDLINE_LINUX=.*usbcore\.quirks=', original, re.M):
    raise SystemExit('Existing USB quirks need manual consolidation before this configuration.')
if quirks:
    words[words.index(quirks[0])] = 'usbcore.quirks=' + merge_quirk(quirks[0].split('=', 1)[1])
else:
    words.append('usbcore.quirks=1532:0577:r')
updated = original[:match.start(2)] + ' '.join(words) + original[match.end(2):]
if updated != original:
    backup = Path('/etc/default/grub.before-makepad-blackshark')
    if not backup.exists():
        shutil.copy2(grub, backup)
    grub.write_text(updated)
    try:
        subprocess.run(['grub-mkconfig', '-o', '/boot/grub/grub.cfg'], check=True)
    except Exception:
        grub.write_text(original)
        raise
parameter = Path('/sys/module/usbcore/parameters/quirks')
parameter.write_text(merge_quirk(parameter.read_text()))
print('BlackShark USB quirk configured for this session and future GRUB boots.')
PY
install -m 0644 "$blackshark_script_dir/70-makepad-blackshark.rules" /etc/udev/rules.d/70-makepad-blackshark.rules
udevadm control --reload-rules
python3 - <<'PY'
from pathlib import Path
import subprocess
for device in Path('/sys/class/hidraw').glob('hidraw*'):
    if '1532:0577' in str(device.resolve()):
        subprocess.run(['udevadm', 'trigger', '--action=change', str(device.resolve())], check=True)
subprocess.run(['udevadm', 'settle'], check=True)
PY
echo 'HID access configured for the active seat and input group. No reboot performed.'
