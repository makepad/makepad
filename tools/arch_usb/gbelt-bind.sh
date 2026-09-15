#!/usr/bin/env bash
# Register FTDI 0403:939b with the installed ftdi_sio driver. No serial I/O.
set -euo pipefail
/usr/bin/modprobe ftdi_sio
new_id=/sys/bus/usb-serial/drivers/ftdi_sio/new_id
if test -w "$new_id"; then
    if ! err=$( { printf '0403 939b\n' >"$new_id"; } 2>&1 ); then
        case "$err" in
            *'File exists'*) ;;
            *)
                printf 'makepad-gbelt-bind: failed to register 0403 939b: %s\n' "${err:-write failed}" >&2
                exit 1
                ;;
        esac
    fi
fi
