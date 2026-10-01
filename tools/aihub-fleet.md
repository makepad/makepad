# AI Hub fleet security

The fleet authority (one P-256 key on the admin's Mac) decides which nodes
serve and which clients may ask. Code: `libs/ai/hub/src/fleet_auth.rs`
(credentials, pins), `libs/ai/hub/src/front.rs` (TLS front),
`makepad_network::tls` (TLS on each OS's own stack).

## What a node exposes

A node bound to a network address (anything but loopback) serves only
through its **TLS front**:

- **Certificate.** The node presents its own self-signed certificate,
  stored in `<cache>/fleet/tls`. The fleet authority endorses it, and the
  endorsement is broadcast in the node's discovery beacon.
- **Authentication.** Every request must carry a fleet credential
  (`Authorization: Bearer mkc1.…`), signed by the authority and not revoked.
  This includes `/health` and model blobs; nothing is open.
- **Roles.**
  - `lan` (people's apps) reaches the whole job API.
  - `node` reaches only model blobs and `/health` (peer transfer).
  - `device` reaches only its own job routes.
- **Ownership.** Jobs belong to the credential that submitted them. Only
  that client can cancel them, keep their leases alive or say `/bye` for
  them.
- **Limits.**
  - 10 s for TLS.
  - A request head of at most 16 KiB that must arrive within 15 s.
  - 5 failed authentications from one address within 10 min ban it for
    15 min.
  - 32 connections per address, 256 in total (8 and 64 on the edge).
- **The node's HTTP server.** It listens on loopback only. It trusts identity
  headers only when they carry the front's per-process secret.
- **Fail closed.** Without `<cache>/fleet/authority.pub` and a valid
  `<cache>/fleet/node.endorsement` matching its node key and certificate, a
  network-bound node refuses to start.
- **Machine nodes.** A loopback-only node (the machine node, tests) is
  machine-local and needs none of this.

## What a client does

- **Discovery.** It trusts only signed beacons: an endorsement that verifies
  under the authority in `~/.makepad/ai-hub/authority.pub`, for the wanted
  fleet. Each verified node is pinned (host:port → certificate SHA-256), and
  its URL is `https://<ip>:<port>`.
- **Credential.**
  - The HTTP client (`http_client`) and websockets (`PlainWebSocket`,
    `wss://`) connect to pinned endpoints over TLS pinned to that
    certificate.
  - Only after the pin check do they send this process's credential:
    `MAKEPAD_AI_HUB_TOKEN`, `~/.makepad/ai-hub/client.token`, or a node's
    `<cache>/fleet/node.token`.
  - A fleet credential is refused for any endpoint that is not pinned, so a
    forged beacon or a typo can't collect one.
- **Peer sources.** A node dials peers only if they are verified roster
  members:
  - request-supplied `peer_sources` must be pinned endpoints;
  - `MAKEPAD_AI_PEER_SOURCES` hosts are mapped onto the endorsed roster.

## Admin tasks (`ai-fleet`, admin Mac only)

```sh
cargo build --release -p makepad-ai-hub --bin ai-fleet
./target/release/ai-fleet init      # once: ~/.makepad/ai-hub/authority/, and trust it here
./target/release/ai-fleet pub       # the authority.pub every node and client gets
```

**Enrol a node.** `makepad-app-ai-hub --fleet-identity --cache-dir <cache>`
on the node prints `node_key=` and `tls_sha256=`. Then:

```sh
ai-fleet endorse gen <node_key> <tls_sha256> > node.endorsement
ai-fleet issue node-<name> node --out node.token
```

Place `authority.pub`, `node.endorsement` and `node.token` in
`<cache>/fleet/` on the node (through the tunnel), readable only by the
node's account, and restart it.

**Credentials for people and apps.** For each, run
`ai-fleet issue <id> lan --out <file>`. Install the file as
`~/.makepad/ai-hub/client.token` (mode 600), next to `authority.pub`.

**Revoke.** Run `ai-fleet revoke <client id or node key>`, copy
`~/.makepad/ai-hub/authority/revoked` to every node's `<cache>/fleet/revoked`
(and to clients, so revoked nodes drop out of discovery), then restart the
nodes. To rotate a credential, issue a new one and revoke the old id.
Credentials and endorsements expire after `--days` (default 365).

## The switch from the open fleet (until 2026-10-15)

The transition is explicit and dated (`fleet_auth::LEGACY_FLEET_UNTIL`):

- A network node **without** credentials still starts, serves plaintext
  without authentication as before, and logs `LEGACY OPEN NODE`.
- A node **with** credentials and `MAKEPAD_AI_HUB_LEGACY_PLAIN=1` serves
  both ways:
  - the TLS front on port + 1 (8124), with the signed beacon;
  - the old plaintext port (8123), with the unsigned beacon.
- A client **with** a fleet credential uses signed nodes over pinned TLS. It
  also accepts unsigned nodes not seen signed, logged as `LEGACY unsigned
  node`. They are never sent a credential, because they are not pinned.
- A client **without** a credential keeps using the legacy nodes.
- Peers named in a request may be pinned nodes, or legacy nodes this node
  has itself discovered.

From the date on, every legacy path refuses. Before then, once every box is
enrolled and every client has switched:
1. Remove `MAKEPAD_AI_HUB_LEGACY_PLAIN` from the launchers. The front then
   takes the node's port and the plaintext server moves to loopback.
2. Delete the legacy code paths.
