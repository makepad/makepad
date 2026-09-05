# AI backends: one broker, six providers, one lock

The asset server's chat broker (`libs/asset/chat` + `libs/asset/store/src/host/chat.rs`)
is the ONLY place a model is reached from. Every app — the sandbox game, the
VJ, the Asset UI — is a thin client of `/v1/chat/*`: it opens a session on a
named provider, sends text, and renders the event stream. No app holds a
key, a fleet address, or a CLI.

## Providers (`GET /v1/chat/providers`)

| slug         | what runs                                    | locality | tools contract |
|--------------|----------------------------------------------|----------|----------------|
| `fleet-qwen` | Qwen on our asset-ai fleet (LAN beacons)      | `local`  | `<<tool>>` text markers |
| `openai`     | OpenAI Responses API (`OPENAI_API_KEY` on the server) | `cloud` | native functions |
| `grok`       | xAI Responses API (`XAI_API_KEY` on the server) | `cloud` | native functions |
| `claude-cli` | the `claude` CLI logged in on the server host | `cloud`  | `<<tool>>` text markers |
| `codex-cli`  | the `codex` CLI on the server host            | `cloud`  | `<<tool>>` text markers |
| `grok-cli`   | the `grok` CLI on the server host             | `cloud`  | `<<tool>>` text markers |

Each row is `{kind, locality, state: available{model} | unavailable{reason}}`.
`locality` is the server's word: `local` means our own fleet, `cloud` means a
vendor — by key or by a logged-in CLI, the client does not care which. A
frontend that promises "Local AI only" filters on that field and nothing
else, so adding a provider never needs a client release to stay honest.

### The CLI providers (`cli.rs`, `claude.rs`, `codex_cli.rs`, `grok_cli.rs`)
- One process per turn, in its own process group, cwd = an empty scratch
  dir under the temp dir; cancel kills the group. Conversation continuity
  is the CLI's own session id (`--resume` / `codex exec resume`), so only
  the new tail of the history is sent on later turns.
- Chat-only by construction: claude `--tools ""` + empty strict MCP set;
  grok `--disallowed-tools <all it advertises>` + `--permission-mode dontAsk`
  + `--max-turns 1`; codex `--sandbox read-only`, user config and rules
  ignored, `shell_environment_policy.inherit=none`. The only tools any of
  them can express are the broker's content tools, executed by the
  dispatcher against the asset server (same as the fleet lane).
- Reasoning is forwarded inside `<think>…</think>` text like the fleet lane,
  so every client renders it one way and none of it enters the history.
- Availability = "is the executable there" (`CLAUDE_CODE_PATH` /
  `CODEX_CLI_PATH` / `GROK_CLI_PATH` override, then `$PATH`, then the usual
  install dirs). The reason string never carries a path to the wire.
- Verified live (2026-08-26, macOS): claude 2.1.246 needs the prompt on
  stdin (a trailing positional after `--tools ""` is swallowed as a tool
  name); grok 1.0.5 ignores `--tools ""`/`none` but honours
  `--disallowed-tools`; codex 0.149.1 `exec --json` emits whole
  `agent_message` items, no deltas.

### Fleet discovery (`fleet_discovery.rs`)
The fleet lane listens for asset-ai beacons on UDP 41830 and keeps only the
fleet named by the server config, else `MAKEPAD_AI_FLEET`, else `default`.
The embedded server inside the Asset UI pins this to the fleet the UI's own
panel shows (`gen`); an empty name used to force `default` and silently
drop every beacon ("no fleet nodes configured" while the panel showed 2/2
up). The listener starts with the broker and a probe in its first seconds
waits for the first beacon instead of reporting an empty fleet.

## Durable per-game conversations (keyed sessions)

The server keeps the chat STATE for its clients. A session created with
both `client_key` and `context_key` is one durable conversation per
`(principal, client_key, context_key)`:

- `client_key` = who is talking — an opaque, display-safe id the app
  chooses (the sandbox sends `ip:<lan-ip>` today; a multiplayer player id
  later). `context_key` = what it is about — the GAME asset id. One
  conversation per (client, game), never shared across games: a game's
  context is context we cannot pollute.
- Key shape (enforced identically by the client crate, `wire::chat_key_ok`,
  and the route): 1..=64 bytes of `[A-Za-z0-9._:@-]` with at least one
  alnum. Never a secret — the bearer token is the authorization.

### Wire
- `POST /v1/chat/sessions` body gains the optional `client_key` and
  `context_key` (both or neither; one alone is a 400). With both it is
  CREATE-OR-RESUME: a LIVE session for that key answers `200` with itself
  (same `session` id, state as it is); a PERSISTED one is rebuilt from
  its transcript under the SAME id on a fresh provider and answers `200`;
  nothing known answers `201` with a fresh session. Without keys:
  today's behaviour (`201`, ephemeral). A live keyed session asked for on
  a different provider/namespace/profile is rebuilt on the requested one
  while idle (its transcript kept), and is `409 busy` mid-turn.
- The session document (`POST`/`GET`/list/cancel) carries `client_key`
  and `context_key` on keyed sessions, omitted otherwise.
- `GET /v1/chat/sessions/{id}/transcript` →
  `{"session":…, "provider":"<slug>", "turn":N, "truncated":bool,
  "messages":[{"role":"user|assistant|system|tool","text":"…"}]}` — the
  conversation as the client should render it: what the session feeds
  its provider, minus the prompt plumbing (thinking stripped, the tool
  reminder and trained call text folded away); each executed tool is one
  `tool` row whose `text` is a short chip title (`world.set_source · ok`)
  with `tool` (dotted name) and `outcome`
  (`ok|unavailable|denied|refused|failed`) alongside. Bounded: the LAST
  128 rows within 192 KiB of text, each row's text clipped to 8 KiB;
  `truncated` says older rows were dropped. Works for unkeyed sessions
  too (memory only). `404` for a session that is not live — a client
  always create-or-resumes first, then reads.
- `DELETE /v1/chat/sessions/{id}` on a keyed session is the client's
  Clear: the worker AND the persisted transcript go, synchronously; the
  next create-or-resume is a fresh conversation (new id).
- Retiring an asset (`DELETE /v1/assets/{id}`) drops every keyed
  conversation whose `context_key` is that asset — live or on disk,
  whoever's — and logs the count.

### Persistence (`libs/asset/store/src/host/chat_store.rs`)
- `<root>/chat/<principal>/<client_key>/<context_key>.jsonl`, keys
  path-encoded (`[A-Za-z0-9._-]` verbatim, else `%XX`: `ip:10.0.0.7` is
  `ip%3A10.0.0.7`). Header line `{"k":"h","v":1,"session","provider",
  "namespace","profile","client_key","context_key","created_ms"}`, then
  one `{"k":"m","role","text","turn"}` per history row exactly as the
  provider sees it (the transcript is rendered on read), and an optional
  `{"k":"p","resume":…}` slot for a provider-native resume id (unused
  today: the threaded provider wrapper does not expose one; a resumed
  session simply replays its history to a fresh provider, which every
  lane — CLIs included — accepts).
- The session worker appends the new tail after every publish
  (`write_all` + `sync_data`); a crash costs at most one torn last line,
  which `load` drops. A resume rewrites the file (temp + rename) with the
  current binding and the history trimmed to the newest 64 rows, so a
  resumed conversation always has room to continue.
- Workers are bounded: a keyed session idle 30 min is evicted to disk;
  when an owner's cap (or the server's) is full of idle keyed sessions,
  the longest-idle one is evicted to make room. Unkeyed sessions are
  never evicted (nowhere to go) and refuse as before.
- One writer per file (the worker) and Clear wipes under the same lock,
  so a late append can never resurrect a cleared transcript.

### `world.new_level` (game profile)
Advertised next to `world.set_source`, client-executed: `{"title",
"source", "note"?}` — "create a NEW game from this source and switch the
player to it". The game publishes, switches, and answers the tool result
`{asset_id, alias, title}`; the broker records the round and ends the
turn with `done` (no further model round — the player is in another game,
which has its own conversation). `world.set_source` stays the in-place
edit of the CURRENT game.

## The sandbox (`apps/sandbox`)
- `fleet_chat.rs` holds `ChatPrefs { provider, local_only }`, persisted in
  `local/sandbox/ai-prefs`, default `{fleet-qwen, true}`. The lock is
  enforced in the worker, not just hidden in the UI: selecting a `cloud`
  provider while locked is refused with a chat line; turning the lock on
  while on a cloud provider drops back to the fleet.
- Settings = one dropdown of what the server listed (cloud rows removed
  while locked; unavailable rows shown as such) + the "Local AI only"
  checkbox. No keys, no pairing, no device-local backends.
