#!/usr/bin/env bash
# Configure the current user's Steam launcher for Makepad WM. Does not install packages.
# Requires Steam, xwayland-satellite and a systemd user manager.
set -euo pipefail

steam_state="${XDG_STATE_HOME:-$HOME/.local/state}/makepad-steam"
steam_units="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
steam_launcher="$HOME/.local/libexec/makepad-steam"
mkdir -p "$steam_state" "$steam_units" "$steam_launcher" "$HOME/.makepad/wm"
# A separately approved, user-local bridge update can fix Steam without changing
# the system driver/package set. Keep using it when regenerating these units.
steam_bridge=/usr/bin/xwayland-satellite
if [[ -x "$steam_launcher/xwayland-satellite-0.8.3" ]]; then
    steam_bridge="$steam_launcher/xwayland-satellite-0.8.3"
fi

cat >"$steam_launcher/steam" <<'LAUNCHER'
#!/usr/bin/env bash
set -euo pipefail
export XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}"
export DBUS_SESSION_BUS_ADDRESS="${DBUS_SESSION_BUS_ADDRESS:-unix:path=$XDG_RUNTIME_DIR/bus}"
: "${WAYLAND_DISPLAY:?Launch Steam from the Makepad WM dock}"
steam_state="${XDG_STATE_HOME:-$HOME/.local/state}/makepad-steam"
mkdir -p "$steam_state"
exec >>"$steam_state/client.log" 2>&1
printf 'WAYLAND_DISPLAY=%s\n' "$WAYLAND_DISPLAY" >"$XDG_RUNTIME_DIR/makepad-steam-wayland.env"
systemctl --user start makepad-steam-xwayland.service
if systemctl --user is-active --quiet makepad-steam.service; then
    # The launcher window may be closed while a game keeps the session alive.
    systemctl --user reload makepad-steam.service
fi
# Keep the launch handle alive while the service runs. The WM stops startup
# feedback on its first mapped window, or on this command's failure/exit.
exec systemctl --user --wait start makepad-steam.service
LAUNCHER
chmod 0755 "$steam_launcher/steam"

cat >"$steam_units/makepad-steam-xwayland.service" <<UNIT
[Unit]
Description=Xwayland bridge for Steam in Makepad WM

[Service]
Type=notify
NotifyAccess=all
Environment=XDG_RUNTIME_DIR=%t
EnvironmentFile=%t/makepad-steam-wayland.env
ExecStart="$steam_bridge" :103 -nolisten tcp
KillMode=control-group
UNIT

cat >"$steam_units/makepad-steam.service" <<'UNIT'
[Unit]
Description=Steam in Makepad WM
BindsTo=makepad-steam-xwayland.service
After=makepad-steam-xwayland.service

[Service]
Type=exec
Environment=DISPLAY=:103
Environment=XDG_RUNTIME_DIR=%t
Environment=DBUS_SESSION_BUS_ADDRESS=unix:path=%t/bus
EnvironmentFile=%t/makepad-steam-wayland.env
WorkingDirectory=%h
ExecStart=/usr/bin/steam
ExecReload=/usr/bin/steam steam://open/main
KillMode=control-group
TimeoutStopSec=45
UNIT

python3 - "$HOME/.makepad/wm/wayland_apps.txt" "$steam_launcher/steam" <<'PY'
from pathlib import Path
import sys
path = Path(sys.argv[1])
lines = path.read_text().splitlines() if path.exists() else []
lines = [line for line in lines if line.split('|', 1)[0].strip().lower() != 'steam']
lines.append('steam | Steam | | "' + sys.argv[2].replace('"', '\\"') + '"')
path.write_text('\n'.join(lines) + '\n')
# Seed the Windows compatibility tool on a fresh Steam profile. Existing
# profiles belong to the user and are never rewritten by this configurator.
config = Path.home() / '.local/share/Steam/config/config.vdf'
if not config.exists():
    config.parent.mkdir(parents=True, exist_ok=True)
    config.write_text('''"InstallConfigStore"
{
    "Software"
    {
        "Valve"
        {
            "Steam"
            {
                "CompatToolMapping"
                {
                    "0"
                    {
                        "name" "proton_experimental"
                        "config" ""
                        "priority" "75"
                    }
                }
            }
        }
    }
}
''')
PY
systemctl --user daemon-reload
printf 'Steam is configured for the Makepad WM dock. The WM stops it after its last window closes.\n'
