#!/usr/bin/env bash
# Password access for the Makepad USB's existing arch account.
# Run `sudo bash tools/arch_usb/ssh.sh enable` to update an existing USB.
set -Eeuo pipefail
export PATH=/usr/local/sbin:/usr/local/bin:/usr/bin
umask 077

action=${1:-show}
case "$action" in
    enable|install|disable|show|reset-password) ;;
    -h|--help) echo 'Usage: sudo makepad-ssh [enable|disable|show|reset-password]'; exit 0 ;;
    *) echo 'Usage: makepad-ssh [enable|disable|show|reset-password]' >&2; exit 2 ;;
esac
if test "$EUID" -ne 0; then
    echo 'Run with sudo (or from the root console).' >&2
    exit 1
fi
state=/var/lib/makepad-ssh
password_file=$state/password
issue=/etc/issue.d/50-makepad-ssh.issue

show_access() {
    printf '\nMakepad network SSH: %s\n' "$(systemctl is-active sshd.service || true)"
    if ! systemctl is-active --quiet sshd.service; then
        printf 'Enable with: sudo makepad-ssh enable\n'
        return
    fi
    printf 'Login: arch (use sudo for root)\n'
    if test -s "$password_file"; then
        printf 'SSH / sudo password: %s\n' "$(cat "$password_file")"
    fi
    local address found=no
    while read -r address; do
        test -n "$address" || continue
        printf '  ssh arch@%s\n' "$address"
        found=yes
    done < <(ip -o address show scope global | awk '{split($4, address, "/"); print address[1]}')
    if test "$found" = no; then
        printf 'Waiting for a network address. Connect Ethernet, or configure Wi-Fi with iwctl.\n'
    fi
    printf 'Show again: sudo makepad-ssh show\nDisable: sudo makepad-ssh disable\n\n'
}

if test "$action" = show; then
    show_access
    exit 0
fi
if test "$action" = disable; then
    systemctl disable --now sshd.service
    rm -f "$issue"
    agetty --reload || true
    echo 'Network SSH disabled. Enable with: sudo makepad-ssh enable'
    exit 0
fi

id arch >/dev/null
install -d -m 0700 "$state"
install -d -m 0755 /usr/local/sbin /etc/ssh/sshd_config.d /etc/issue.d /etc/profile.d
if test "$(readlink -f "$0")" != /usr/local/sbin/makepad-ssh; then
    install -m 0755 "$0" /usr/local/sbin/makepad-ssh
fi

# Draw a fresh passphrase on the target, never in a shared image or build log.
# Use the user's requested two-word, dash-separated format.
if test ! -s "$password_file" || test "$action" = reset-password; then
    python3 - <<'PY' > "$state/password.new"
import secrets

words = '''
acorn amber apple apron arrow atlas bacon badge bagel baker beach berry birch bloom blue boat
boots bread brick brook brush cabin camel candy canoe cedar chalk charm cherry chess chili cloud
clover cocoa coral crane creek crown daisy dance dawn deer delta diner dough dream drift drum
eagle earth ember fairy fern field finch flame flute forest fox frost fruit garden gecko glass
globe grape grass green grove guitar hazel heart heron honey horse house ice iris ivory jacket
jade jazz jelly jewel jolly juice kite kiwi lake lamp laser lemon lilac lime linen lion
lotus lucky lunar mango maple marble melon mint mist moon moss mouse music navy nest night
ocean olive onion orbit otter owl panda paper peach pearl pebble pepper piano pine plum polar
'''.split()
print('-'.join(secrets.choice(words) for _ in range(2)))
PY
    chmod 0600 "$state/password.new"
    mv "$state/password.new" "$password_file"
fi
# Also retries an interrupted setup without losing the displayed password.
printf 'arch:%s\n' "$(cat "$password_file")" | chpasswd
chage -E -1 -I -1 -M 99999 arch

cat > "$state/sshd.conf" <<'EOF'
PubkeyAuthentication no
PasswordAuthentication yes
KbdInteractiveAuthentication no
AuthenticationMethods password
UsePAM yes
PermitEmptyPasswords no
PermitRootLogin no
AllowUsers arch
Banner none
EOF
# OpenSSH uses the first value obtained, so the managed policy goes first.
install -m 0644 "$state/sshd.conf" /etc/ssh/sshd_config.d/00-makepad.conf
if ! head -1 /etc/ssh/sshd_config | grep -Fxq 'Include /etc/ssh/sshd_config.d/00-makepad.conf'; then
    { printf 'Include /etc/ssh/sshd_config.d/00-makepad.conf\n'; cat /etc/ssh/sshd_config; } > "$state/sshd_config.new"
    install -m 0600 "$state/sshd_config.new" /etc/ssh/sshd_config
fi
ssh-keygen -A
sshd -t
sshd -T -C user=arch,host=makepad-arch,addr=127.0.0.1 > "$state/sshd-effective.txt"
for policy in 'pubkeyauthentication no' 'passwordauthentication yes' 'kbdinteractiveauthentication no' 'usepam yes' 'permitrootlogin no' 'permitemptypasswords no' 'authenticationmethods password' 'allowusers arch' 'banner none'; do
    grep -Fxiq "$policy" "$state/sshd-effective.txt"
done
# This is the account/key file owned by the USB builder, not the Mac's key.
rm -f /home/arch/.ssh/authorized_keys

# agetty reads this as root. It is deliberately absent from /etc/motd and
# the SSH Banner: network logins must not reveal the console passphrase.
printf 'Makepad Arch USB - login: arch\nIPv4: \\4\nRun makepad-status for package setup progress.\n\n' > /etc/issue
printf 'Makepad Arch USB\nSSH / sudo password is shown on the local console.\nRun sudo makepad-ssh show to display connection details.\nRun makepad-status for package setup progress.\nWi-Fi connections are configured with iwctl.\n' > /etc/motd
chmod 0644 /etc/issue /etc/motd
{
    printf '\nMakepad network SSH - login: arch (sudo for root)\n'
    printf 'SSH / sudo password: %s\n' "$(cat "$password_file")"
    printf 'Connect: ssh arch@\\4\n'
    printf 'Show addresses: sudo makepad-ssh show\n'
    printf 'Disable: sudo makepad-ssh disable\n\n'
} > "$issue"
chmod 0600 "$issue"
cat > /etc/profile.d/makepad-ssh.sh <<'EOF'
# Show credentials only on a local, interactive root console, never SSH.
if [ "$(id -u)" = 0 ] && [ -z "${SSH_CONNECTION:-}" ]; then
    case "$-" in
        *i*) case "$(tty 2>/dev/null)" in
            /dev/tty[0-9]*|/dev/console) /usr/local/sbin/makepad-ssh show ;;
        esac ;;
    esac
fi
EOF
chmod 0644 /etc/profile.d/makepad-ssh.sh
systemctl --root=/ unmask sshd.service
systemctl --root=/ enable sshd.service
agetty --reload || true
if test "$action" != install; then
    systemctl reload-or-restart sshd.service
    show_access
    # Also update an already logged-in root TTY when enabling remotely.
    # Login prompts use issue.d; future root logins use the profile hook.
    current_tty=$(tty 2>/dev/null || true)
    while read -r console; do
        if test "/dev/$console" != "$current_tty" && test -c "/dev/$console"; then
            show_access > "/dev/$console"
        fi
    done < <(who | awk '$1 == "root" && $2 ~ /^tty[0-9]+$/ {print $2}' | sort -u)
fi
