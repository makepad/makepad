# Fleet identity

One identity system for every networked service we run: the AI hub (fleet
front, control, the Claude service), Desk Link (WM, Windows, macOS hosts and
the viewer), relaxhub, the asset store's sync and Sandbox rooms.

Code: `makepad_network::fleet` (`platform/network/src/fleet/`): `id.rs`
(keys, credentials, proofs, rights, announcements, revocation), `front.rs`
(the TLS front of an HTTP service), `lan.rs` (the private-LAN bind rule);
`makepad_network::tls` (TLS on each OS's own stack, the certificate with
the fleet extension). The tool is `fleet-id`
(`cargo build --release -p makepad-network --bin fleet-id`).

## The model

- **Every machine has its own key**: `~/.makepad/fleet/id.key` (P-256,
  0600; `%USERPROFILE%\.makepad\fleet` on Windows; `MAKEPAD_FLEET_DIR`
  overrides). It never leaves the machine once installed and is never in a
  repo, worktree, fixture, log or command output.
- **A credential names it**: `~/.makepad/fleet/id.cred`,
  `mkid1.<name>.<role>.<expiry>.<from>.<public key>.<root signature>`,
  signed by the fleet **root** key that lives only on the admin Mac
  (`~/.makepad/fleet/admin/root.key`). Credentials hold no secret.
  Roles: `device` (the admin Mac), `node` (WM/Linux machines, the Windows
  box, the mini), `paired` (a phone paired with a hub). `from` is the
  address the credential's admin rights hold from (the Mac's: 10.0.0.222).
- **Every machine checks against** `~/.makepad/fleet/root.pub` and
  `~/.makepad/fleet/revoked` (one name per line). Revoking a name disables
  that machine only; the others keep working.
- **Servers prove themselves in the TLS handshake**: their certificate is
  made from `id.key` and carries their credential in an X.509 extension.
  A client checks it against `root.pub` (signature, key match, expiry,
  revocation): no first-contact pinning, no `known_nodes`.
- **Clients prove themselves without sending a secret**: the HTTP
  `Authorization` value (and Desk Link's hello) is
  `MKID <credential>.<unix time>.<ECDSA(id.key, "mkid1 proof|<server cert sha256 hex>|<time>")>`.
  It is useless against any other server and goes stale after 5 minutes
  (machine clocks must be within that).
- **Rights.** Services call `fleet::id::require(&peer, Need::…)`:
  - `Need::Admin`: role `device` AND the connection comes from the address
    bound into that credential. Both, always. Admin rights: Desk Link
    viewing; relaxhub master (inspect, code push, WM restart, replies); the
    AI hub's control panel, control routes and the Claude service.
  - `Need::Machine`: a node, or the Mac's credential from another address:
    what node-to-node traffic needs (AI hub job routing, kernels, model
    blobs and inventories, relaxhub clustering, store sync).
  - `Need::ThisMachine`: the service's own credential (same name and key
    as its certificate's), i.e. another process of the same machine. The
    AI hub gives it the machine-local control its WM needs (`GET /control`
    and its watch, `/control/local-use`, `/control/yield`, `/control/model`);
    never admin settings. Proven by the key, never by address.
  - Paired phones reach their own jobs and the on-device model files.
- **Announcements** (AI hub beacons, Sandbox room ads, relaxhub discovery)
  are signed by the sender's key with its credential attached
  (`Me::announce`, `Fleet::check_announcement`); receivers drop anything
  that does not check out.
- **No open services.** A service bound to a network address serves only
  through its TLS front and refuses to start without `id.key`, `id.cred`
  and `root.pub`. A loopback-only service (the machine's own hub) is
  machine-local: its callers are admin.
- **Front limits** (`front.rs`): 10 s for TLS; a request head of at most
  16 KiB within 15 s; 5 failed authentications from one address within
  10 min ban it for 15 min; 32 connections per address, 256 in total (8
  and 64 on the edge).

## Admin tasks (`fleet-id`, admin Mac)

```sh
fleet-id init                                    # once: admin/root.key + root.pub
fleet-id issue n217 node --out /tmp/n217         # a machine's files, to ship over ssh
fleet-id sign <name> <role> <point> [--from ip]  # for a key made on its machine (fleet-id keygen there)
fleet-id revoke n217                             # then ship `revoked` and restart services
fleet-id show                                    # this machine's credential, valid or not
```

`local/agent_state/relax-campaign/fleetid-data/issue-credentials.sh` issues
and installs every machine's credential through the ssh path, ships
`revoked` (`--revoked`) and deletes the retired symmetric secrets
(`--retire-old`). It prints no secrets.

**Scripts without the Rust client** need ECDSA P-256: sign
`mkid1 proof|<sha256 hex of the server's DER certificate>|<unix time>` with
the machine's key (SHA-256, raw r||s as 128 hex digits) and send
`Authorization: MKID <id.cred text>.<time>.<signature>`.

## Going on the internet later (not done; for the user)

The hub on 165 can serve an **edge**: a second TLS front with device routes
only. Set `MAKEPAD_AI_HUB_EDGE=0.0.0.0:8443` in its environment.

**Router (OpenWrt at 10.0.0.1).** Forward WAN tcp/8443 to the hub only:

```sh
uci add firewall redirect
uci set firewall.@redirect[-1].name='aihub-edge'
uci set firewall.@redirect[-1].src='wan'
uci set firewall.@redirect[-1].src_dport='8443'
uci set firewall.@redirect[-1].dest='lan'
uci set firewall.@redirect[-1].dest_ip='10.0.0.165'
uci set firewall.@redirect[-1].dest_port='8443'
uci set firewall.@redirect[-1].proto='tcp'
uci commit firewall && /etc/init.d/firewall reload
```

Never forward the nodes' ports or :8384 (tunnels).

**Address.** For a dynamic home IP, set up DDNS on the router
(`opkg install ddns-scripts` plus a provider package; LuCI → Services →
Dynamic DNS) for a hostname of your choice, or use a static IP.

**Devices.**
- On the LAN, issue a credential per device:
  `fleet-id issue <phone-name> paired --out …`.
- Enrolment should carry it with the hub's address and certificate
  fingerprint, e.g. a QR code with
  `makepad-hub://<host>:8443?fp=<sha256>&cred=<credential>`.
- On the edge, devices must pin the hub certificate **strictly**: refuse a
  change, unlike the LAN's warn-and-continue rule, because a device on the
  internet has no other way to tell the hub apart. This is still to be
  decided and built, with the enrolment UI.

**Checks before opening the port.** From outside the LAN (a phone hotspot):
- the forwarded port answers TLS only;
- a request without a credential gets 401, and repeated failures ban that
  address;
- only device routes work.
