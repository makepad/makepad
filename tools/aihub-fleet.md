# AI Hub fleet security

Code: `libs/ai/hub/src/fleet_auth.rs` (credentials), `libs/ai/hub/src/front.rs`
(the TLS front), `makepad_network::tls` (TLS on each OS's own stack, known
hosts).

## The model

- **Nodes are trusted the ssh way.** Each node makes its own self-signed
  certificate on first start (`<cache>/fleet/tls`). A client records the
  certificate fingerprint per `host:port` on first contact, in
  `~/.makepad/ai-hub/known_nodes`. If it later changes, the client:
  - prints a loud warning (host, old and new fingerprint);
  - appends it to `known_nodes.log`;
  - keeps it in `makepad_network::tls::fingerprint_changes()`, for status
    displays;
  - updates the record and continues.

  Nothing about nodes is provisioned.
- **Clients are what is checked.**
  - A fleet key (32 random bytes) is on the admin Mac and on every node
    (`<cache>/fleet/fleet.key`, 0600).
  - A client credential is `mkc2.<id>.<role>.<expiry>.<secret>`, where the
    secret is derived from the fleet key.
  - The credential is never sent. Each request carries
    `Authorization: MKC2 <id>.<role>.<expiry>.<proof>`, where the proof is
    HMAC(secret, `"mkfleet2 proof|" + certificate sha256`) for the
    certificate the node actually presented.
  - Something in the middle with its own certificate only gets a proof for
    that certificate, which is useless against a real node.
- **Roles and ownership.**
  - `lan` (people's apps) reaches the whole job API.
  - `node` reaches model blobs and `/health` only.
  - `device` reaches its own job routes only.
  - Jobs belong to the credential that submitted them. Only that client can
    cancel them, keep their leases alive or say `/bye` for them.
- **No open nodes.** A node bound to a network address serves only through
  its TLS front, and refuses to start without `<cache>/fleet/fleet.key`. A
  loopback-only node (the machine node, tests) is machine-local and needs
  none of this.
- **Front limits.**
  - 10 s for TLS.
  - A request head of at most 16 KiB, within 15 s.
  - 5 failed authentications from one address within 10 min ban it for
    15 min.
  - 32 connections per address, 256 in total (8 and 64 on the edge).
- **The node's HTTP server.** It listens on loopback only and trusts
  identity headers only when they carry the front's per-process secret.
- **Discovery.** Beacons (`makepad-ai-hub-tls`) only say where a node's TLS
  port is. A process with a credential reaches every node it hears; one
  without a credential ignores them (logged).
- **Peers.** A node fetches model blobs from fleet endpoints only:
  - request `peer_sources` must be discovered fleet endpoints;
  - `MAKEPAD_AI_PEER_SOURCES` hosts are mapped onto the discovered nodes.

## Admin tasks (`ai-fleet`, admin Mac only)

```sh
cargo build --release -p makepad-ai-hub --bin ai-fleet
./target/release/ai-fleet init                                  # ~/.makepad/ai-hub/admin/fleet.key
./target/release/ai-fleet issue rik-mac lan --out ~/.makepad/ai-hub/client.credential
./target/release/ai-fleet node-files w203 --out /tmp/w203       # fleet.key + node.credential
./target/release/ai-fleet revoke <client id>
```

**Enrol a node.** Copy `fleet.key` and `node.credential` (from
`node-files`) into `<cache>/fleet/`, readable only by the node's account,
and restart the node.

**Enrol a person or app.** Run `ai-fleet issue <id> lan --out <file>`. On
the client, the file becomes `~/.makepad/ai-hub/client.credential` (mode
600), or `MAKEPAD_AI_HUB_CREDENTIAL` holds its text.

**Revoke.** `ai-fleet revoke <id>` adds the id to
`~/.makepad/ai-hub/admin/revoked`. Copy that file to every node's
`<cache>/fleet/revoked` and restart the nodes. Credentials expire after
`--days` (default 365); to rotate one, issue a new one and revoke the old
id. To rotate the fleet key itself, run `ai-fleet init` on a fresh admin
directory, then re-issue everything.

**Scripts without the Rust client** (for example Python):
1. Open TLS without CA checking.
2. Take sha256 of the server's DER certificate. Compare it with your own
   record if you keep one; warn on a change and continue.
3. Send `Authorization: MKC2 <id>.<role>.<expiry>.<hex HMAC-SHA256(bytes.fromhex(secret), b"mkfleet2 proof|" + fingerprint_hex)>`.

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
  `ai-fleet issue <phone-name> device --out …`.
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
