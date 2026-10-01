# The remote tunnel

`makepad-remote --server` (port 8384) lets an authorised client push files,
run `cargo` or shell commands, start background jobs and read logs on a build
or GPU box. Clients: `makepad-remote <host> …`, `cargo makepad tunnel <host>:8384 …`,
`mapfleet`, and the CI tools.

## Security model

- **Transport.** TLS on the operating system's own stack (Schannel on
  Windows, SecureTransport on macOS, OpenSSL on Linux; see
  `platform/network/src/tls.rs`). TLS 1.2 or later, ECDHE-ECDSA with an AEAD
  cipher only. TLS 1.3 is used where both ends support it.
- **Server identity.** Each box has a self-signed P-256 certificate in its
  identity directory (`tls-key.x963`, `tls-cert.der`,
  `tls-fingerprint.txt`). Clients record its SHA-256 per host on first
  contact (`~/.makepad/tunnel/pins`, like ssh known_hosts). If it later
  changes, the client:
  - prints a loud warning;
  - appends it to `pins.log`;
  - updates the record and continues.
- **Client authentication.** One random 32-byte key per box. Inside TLS the
  client proves it holds the key with HMAC-SHA256 over a fresh server nonce
  and the fingerprint of the certificate it was actually shown. A captured
  proof can't be replayed, and an impersonator with another certificate
  can't use it against the real box (`platform/network/src/tunnel.rs`).
- **Fail closed.** The server refuses to start without a readable key file,
  and on unix it refuses key files that other users can read.
- **Scope.** The server binds the LAN address of its default route (`--bind
  lan`) and refuses public addresses, including `0.0.0.0`. It drops peers
  outside private, loopback and link-local ranges. The Windows firewall rule
  allows the local subnet only.
- **Throttling.** Five authentication failures from one address within 10
  minutes ban that address for 15 minutes. At most 8 handshakes can be
  pending at once, and each must finish within 15 s. Port probes (a connect
  and close) and clients that predate TLS are refused without counting
  toward a ban.
- **Audit log.** Every connection, authentication result, ban and request
  (with the key id) goes to `audit.log`, with control characters escaped.
  Keys are never logged.
- **Time limit.** A foreground `cargo` or `shell` run is stopped after 6 h
  (`--max-run <secs>`). Spawned jobs are meant to outlive the connection and
  have no limit.
- **Admin allowlist.** The server never runs anything elevated. The fixed
  actions `node-status`, `node-stop`, `node-start`, `node-restart` and
  `tunnel-restart` (`makepad-remote <host> admin <action>`) take no client
  text. On the Windows service layout the tunnel's account may start, stop
  and query `MakepadAiNode` and no other service.

What an authenticated client can do: everything the tunnel's account can do
(it runs arbitrary commands with `--all`). That is why the account is not an
administrator, and why each key is per box and rotatable.

## Files

Client (this Mac), in `~/.makepad/tunnel/` (or `$MAKEPAD_TUNNEL_DIR`):
- `psk`: `<host> <64-hex key>` lines, mode 600. Two lines for one host mean
  a rotation is in progress; they are tried in order.
- `pins`: known hosts, `<host> <64-hex certificate sha256>`. Recorded on
  first contact; `pins.log` lists changes.
- `outbox/<host>.keys`: the server key file `keygen` writes for a new box.
  Delete it once installed.

Server:
- per-user layout: `%USERPROFILE%\.makepad\tunnel\` or `~/.makepad/tunnel/`,
  holding `server-keys`, `identity/` and `audit.log`.
- Windows service layout: `C:\ai\services\tunnel\`, holding
  `makepad-remote.exe`, `server-keys`, `identity\`, `audit.log` and
  `tunnel.log`. Only SYSTEM, Administrators and `NT SERVICE\MakepadTunnel`
  can read it.

## Common tasks

```sh
makepad-remote 10.0.0.203 shell 'echo hi'          # port 8384 by default
cargo makepad tunnel 10.0.0.203:8384 --no-sync run script.ps1
makepad-remote 10.0.0.203 admin node-restart
makepad-remote rotate 10.0.0.203                   # new key; no lock-out window
```

`rotate` works without a window in which either side could lock the other
out. Over a session using the current key, it sets the server to accept
{new, current}. It then stores {new, current} on the client, proves the new
key in a fresh session, drops the current key on the server over that
session, and finally drops it on the client. The server rejects any key set
that does not contain the key of the session sending it.

**Identity rotation** (a new certificate). Over the current pinned session:
1. Run `makepad-remote --init --identity <dir>.next` on the box.
2. Pull `<dir>.next/tls-fingerprint.txt`.
3. Move `<dir>.next` into place.
4. Run `admin tunnel-restart`.
5. The client records the new fingerprint on its next connect and warns
   once. `makepad-remote pin <host> <hex>` records it ahead of time
   instead.

## Adding a box

1. On the client, run `makepad-remote keygen <host>`. It stores the client
   key and writes `outbox/<host>.keys`.
2. On the box, install that file as the server's `server-keys`, readable by
   the server's account only.
3. Start `makepad-remote --server --all`, or on Windows run
   `tools/aihub-node-install-services.ps1` (see `tools/aihub-farm.md`).

If the key file had to cross the network in the clear (a first install with
no tunnel yet), run `makepad-remote rotate <host>` right after. The new key
then only ever travels inside TLS, which uses ephemeral key exchange.

## macOS boxes (the CI Mac mini)

The tunnel runs as the LaunchAgent `nl.makepad.tunnel`, from
`~/.makepad/tunnel/bin/makepad-remote`, in the bridge checkout, with
`MAKEPAD_REMOTE_SUPERVISED=1` so `tunnel-restart` exits and launchd
restarts it.

Least privilege still needs an administrator: the agent runs as `dev`, who
is an admin. Moving it to a standard user means:
1. Create a standard user.
2. Give that user its own rustup/cargo, and ACLs on the bridge checkout
   (`chmod -R +a "<user> allow read,write,…"`).
3. Turn the agent into a LaunchDaemon with `UserName` set to that user.
