#!/bin/sh
# install-hotspot-network.sh -- the one root-owned file the WM's Wi-Fi
# hotspot needs (see apps/wm/src/shell/hotspot_linux.rs, "Addressing").
#
# What it does: writes a systemd-networkd drop-in that gives the access-point
# interface 192.168.77.1/24 and turns on networkd's built-in DHCP server for
# it, then reloads networkd. No package, no new daemon. It does NOT start the
# hotspot and does not touch the current Wi-Fi connection: the file only
# matches an interface in access-point mode. networkd uses the first matching
# file in name order, so this 20- file must sort before the station profile
# that matches the same interface (here 25-makepad-wireless.network, Name=wl*):
# in station mode it does not match and the station profile applies as before.
#
# Use (as root, on the machine that runs the WM):
#   sudo sh install-hotspot-network.sh            install and reload
#   sudo sh install-hotspot-network.sh --remove   remove and reload
#
# Then open the WM's Wi-Fi panel: the Hotspot switch is enabled once this file
# exists. Turning the hotspot on REPLACES the Wi-Fi connection (this radio
# cannot do both): every network session over Wi-Fi drops until it is turned
# off again, which rejoins the previous network.
#
# First live test, from a local terminal (not over Wi-Fi), with an automatic
# way back after three minutes:
#   (sleep 180; iwctl ap wlan0 stop; iwctl device wlan0 set-property Mode station) &
# Undo by hand at any time:
#   iwctl ap wlan0 stop
#   iwctl device wlan0 set-property Mode station
#   iwctl station wlan0 connect "<previous network>"
set -eu

FILE=/etc/systemd/network/20-makepad-hotspot.network

if [ "$(id -u)" != 0 ]; then
    echo "run as root: sudo sh $0 ${1:-}" >&2
    exit 1
fi
if ! systemctl is-active --quiet systemd-networkd; then
    echo "systemd-networkd is not running; the hotspot's addressing needs it" >&2
    exit 1
fi

case "${1:-}" in
--remove)
    rm -f "$FILE"
    networkctl reload
    echo "removed $FILE"
    exit 0
    ;;
"") ;;
*)
    echo "usage: sudo sh $0 [--remove]" >&2
    exit 2
    ;;
esac

mkdir -p /etc/systemd/network
# BEGIN networkd drop-in (must equal hotspot_linux::networkd_dropin(); a unit test checks it)
cat > "$FILE" <<'NETWORK'
[Match]
WLANInterfaceType=ap

[Network]
Address=192.168.77.1/24
DHCPServer=yes
ConfigureWithoutCarrier=yes

[DHCPServer]
EmitDNS=no
NETWORK
# END networkd drop-in
chmod 0644 "$FILE"
networkctl reload
echo "installed $FILE; the WM's Hotspot switch is now available"
