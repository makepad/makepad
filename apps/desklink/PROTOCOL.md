# Desk Link protocol, version 8

Desk Link is how a viewer (this app) sees and drives a desktop (the window manager's
`desk_link` server, `apps/wm/src/desk_link/`; the macOS / Windows Desk Host; a JIT host serving
its own render through the deskhost engine, `makepad_desk_link::host`). It runs between machines
on one LAN only: no relays, no NAT traversal, no discovery service. The one implementation of
the format is `libs/desk_link`, used by both ends.

## Finding desktops

The viewer never broadcasts or listens for announcements (on macOS that asks the Local Network
question). It lists, in this order:
- `--connect host[:port]` (repeatable; `--connect-only`: nothing else);
- its saved hosts, `~/.makepad/desklink/hosts` (`[name] host[:port]` lines);
- the devhub's JIT hosts: an embedded devhub node (a headset, a phone) reports its Desk Link
  endpoint in the `caps` the Mac's devhub master asks for every 10 s (`desk_link`:
  `{"on","port","note"}`; `libs/devhub/src/embed.rs`). The master writes the listening ones to
  `~/.makepad/devhub/desk-links`, one `<node> <ip>:<port>` line each, and the viewer adds those as
  desktops named by node (`quest`), at launch and once a second after (a tab stays when its
  endpoint goes off and shows it offline). The master already dials every node from its
  `nodes.conf` over fleet TLS, so nothing new listens or announces.

`makepad devhub desk <node> on|off` switches a JIT host's endpoint (the node's `desk` service,
through the master's router; `desk <node>` says what it does). The host registers the switch with
`makepad_devhub::embed::set_desk_link_control`. This machine itself is never one of the
viewer's desktops.

## Transport and security

- TCP, default port **8790**, wrapped in TLS on each OS's own TLS stack (`makepad_network::tls`).
- The desktop presents a certificate made from its fleet key (`~/.makepad/fleet/id.key`) that
  carries its fleet credential (tools/fleet-identity.md). The viewer checks it against the fleet
  root (`root.pub`): signature, key match, expiry, revocation. Nothing is pinned; a desktop that
  is not a machine of this fleet is refused before the viewer sends anything.
- The desktop serves only when **all** of these hold. Otherwise nothing listens:
  - its settings enable it (`~/.makepad/wm/desk-link.conf` or `~/.makepad/deskhost.conf`,
    `enabled = on`);
  - the bind address is loopback, a private LAN address or `0.0.0.0`;
  - its fleet identity (`id.key`, `id.cred`, `root.pub` in `~/.makepad/fleet`) loads. A Desk
    Host waits for it, checking every 10 s.
- **Before TLS.** A peer that is not a LAN address (loopback, RFC 1918, link-local, IPv6 ULA) is
  closed unread.
  - Refusals are logged with the address only, at most once a minute per address.
  - Five failed credentials within a minute ban the address for 15 minutes. Sessions are capped
    at 8, with at most 4 per address.
- **Authentication.** The viewer's first message carries its fleet proof:
  `MKID <credential>.<unix time>.<ECDSA-P256(id.key, "mkid1 proof|" + certificate sha256 hex + "|" + time)>`.
  - The proof is bound to the certificate the desktop presented and to the time (5 minutes),
    so a proof captured by anything else is useless. No secret crosses the wire.
  - The desktop checks the credential against its fleet root (signed, unexpired, not revoked)
    and requires the **admin**: the admin Mac's `device` credential, on a connection from the
    address bound into that credential.
  - Anything else is answered `Refused` with the reason and closed.

## Framing

Every message is:

```
type    u8
length  u32 LE        payload bytes (at most 16 MiB; more closes the connection)
payload
```

Fixed-size fields are little-endian. `str` and `bytes` are `u32 LE length` + bytes (str = UTF-8).
`bool` is a `u8` (0 or 1).

## Viewer -> desktop

| type | name | payload |
|---|---|---|
| 1 | Hello | `version u16, name str, authorization str, screen i32 (-1 = the desktop's default), paused bool, caps u32` (1 = the viewer's decoder takes 4:4:4: H.264 High 4:4:4 Predictive, and with 4 H.265 Format range extensions 4:4:4; 2 = view-only: its `Window` never decides the size of a desktop that follows its viewer, e.g. a probe; 4 = it decodes H.265) |
| 2 | Pause | — no pictures for this session; pings continue |
| 3 | Resume | — pictures again, starting with a key frame |
| 4 | SelectScreen | `index u32` (into `Welcome.screens`) |
| 5 | RequestKeyframe | — (sent by the viewer after a decode error) |
| 7 | Refresh | `hz u16`: the viewer's display refresh, measured from its vsync (0 = not known). Sent after Hello and on every change. |
| 8 | Window | `width u32, height u32, scale u16`: the viewer's picture area in device pixels and its scale (hundredths, 200 = 2.00x). Sent once it has held still for 250 ms, after the Welcome and after every resize. A desktop whose size follows its viewer (a virtual display set to follow) takes this size and scale from the viewer that connected last ("last connection wins", see Sessions), so its picture is shown 1:1; the others see it scaled. |
| 10 | Key | `down bool, keycode u32 (makepad KeyCode index), mods u8 (1 shift, 2 control, 4 alt, 8 logo)` |
| 11 | Pointer | `x u16, y u16`: position over the stream picture, 0..=65535 on each axis |
| 12 | Button | `down bool, button u8 (0 left, 1 right, 2 middle, 3 back, 4 forward)` |
| 13 | Scroll | `dx i32, dy i32` in 1/120 notches (a notch = 40 points; fractions carried, so trackpad pixels and momentum stay smooth), in content direction: +dy = the view moves up the content, +dx = right (see Scroll direction) |
| 14 | Gamepad | `slot u8, buttons u32, left_x i16, left_y i16, right_x i16, right_y i16, left_trigger u8, right_trigger u8` |
| 15 | GamepadGone | `slot u8` |
| 20 | Ping | `client_ns u64` (the viewer's clock) |
| 21 | Stats | `frame u32, decode_us u32, present_us u32`: about once a second; `frame` the last picture shown and `present_us` its decode-end to on screen; `decode_us` a picture's decode (its slowest stream's), the mean over the pictures since the last `Stats` (version 7 viewers before this rule sent the last picture's) |
| 22 | Clipboard | `text str` (the desktop's clipboard takes it) |
| 24 | Encoding | `chroma u8, codec u8, preset u8, mbps u16, fps u16`: how this viewer wants the stream encoded, 0 = auto each. `chroma` 1 4:4:4, 2 4:2:0; `codec` 1 H.264, 2 H.265; `preset` 1 sharp (the encoder's best pictures per bit), 2 fast (its fastest encode); `mbps` the most the stream may take (all tiles; 0 = no cap of its own: the desktop streams at most the lowest cap of its watching viewers); `fps` pictures a second (0 = its display's refresh). Sent after the Hello and on every change. The choices of the watching viewer that connected last decide the stream (view-only viewers last); the decoders of every watching viewer (`Hello.caps`) and the desktop's settings bound them. |

Gamepad buttons: bit 0 A, 1 B, 2 X, 3 Y, 4 left shoulder, 5 right shoulder, 6 select, 7 start, 8 home,
9 left thumb, 10 right thumb, 11-14 d-pad up/down/left/right. Sticks run −32767..32767, up positive
(makepad's convention); triggers 0..255.

## Desktop -> viewer

| type | name | payload |
|---|---|---|
| 100 | Welcome | `version u16, session u32, screens (n u32, then per screen: name str, x u32, y u32, width u32, height u32), screen u32, stream_w u32, stream_h u32, input bool, input_note str, server str` |
| 101 | Refused | `reason str` (then the desktop closes) |
| 102 | Video | `frame u32, key bool, built_ns u64, encoded_ns u64, sent_ns u64, stream u8, parts u8, x0 u32, y0 u32, codec u8 (1 H.264, 2 H.265), data bytes` (see Streams) |
| 103 | Cursor | `x u16, y u16 (over the stream picture), shape u8, visible bool` |
| 104 | Pong | `client_ns u64 (echoed), server_ns u64` |
| 105 | Stream | `screen u32, stream_w u32, stream_h u32` (the stream shows another screen now) |
| 106 | Clipboard | `text str` |
| 107 | Bye | `reason str` (then the desktop closes) |
| 108 | Rate | `fps u16, kbps u32, limit u8, full_chroma bool, codec u8, preset u8, streams u8, why str`: the pictures a second the desktop streams at now (the lowest of its settings' cap (`fps`, default 120), its streamed screen's refresh and every watching viewer's `Refresh`; a still desktop sends fewer), the stream's rate (kbit/s, all tiles; 0 = by picture size), what holds the rate (0 starting, 1 nothing: rising, 2 the link, 3 the encoder, 4 a cap, 5 a fixed setting), whether it is 4:4:4, its codec (1 H.264, 2 H.265), the encoder's preset (1 sharp, 2 fast), the encoder streams a picture is cut into, and `why`: what the stream is and why, in words (e.g. "fast: the encoder was behind at sharp"). Sent after Welcome and on every change. `fps=0` with nonempty `why` also carries a desktop preparation/status notice, including before Welcome while the host makes its screen available; viewers show it over the picture until a normal Rate clears it. |

Screens are in desktop device pixels. `input = false` means viewer input is not injected (for
example, no `/dev/uinput` access); `input_note` says why.

## Video

- **Codec.** H.265 when the desktop's settings allow it (`codec = auto`, the default), its GPU
  encodes it and every watching viewer's Hello says it decodes it (caps 4), and the active
  viewer's `Encoding` does not ask for H.264: Main (4:2:0 8-bit) or Format range extensions
  4:4:4 8-bit. Else H.264 High, 4:2:0 8-bit, or High 4:4:4 Predictive (4:4:4 8-bit, CAVLC).
  4:4:4 when the desktop's settings ask for it (`chroma = 444`, the default) and every watching
  viewer's Hello says it decodes it (caps 1). BT.709 limited range (written in the SPS VUI),
  Annex-B access units (one picture each). Each `Video` names its codec; a change of codec
  starts with a key frame in the new one.
  - Every key frame (IDR) starts with its parameter sets (H.264: SPS and PPS; H.265: VPS, SPS
    and PPS).
  - There are no B frames. Every P frame references the picture before it, and output order is
  decode order (POC type 2).
  - The VUI says `max_num_reorder_frames = 0` and `max_dec_frame_buffering = 1`, so decoders output
  each picture as soon as it is decoded.
- **GOP.** Open-ended: an IDR comes at the start, after `Resume`, after `RequestKeyframe` and
  after a screen change.
- **Resolution.** The picture is the selected screen at its own output size. Width is rounded
  down to a multiple of 4 and height to a multiple of 2; the coded size is padded to macroblocks
  and cropped in the SPS. The viewer never renegotiates resolution: it scales the picture to its
  window, aspect-locked and letterboxed, or shows it 1:1. It maps input back through the same
  placement.
- **Damage-driven.** A picture is made only when the desktop's own frame changed (not for a cursor
  move), at most one per `1/fps` seconds (`fps = 60` by default). A change that comes too soon is
  encoded when it is due. An idle desktop sends nothing.
- **Rate control.** VBR at one rate for the whole stream, split over the tiles by area. A fixed
  `kbps` setting, or adaptive: it starts at 250 Mbit/s, goes down below what got out when a viewer falls behind
  (link-bound), and creeps back up when clean, with no ceiling but the desktop's `max_mbps` setting (none by default) and
  the viewers' `Encoding` caps. When the encoder cannot keep a picture's time (its slowest
  tile takes most of it and frames wait for it), that size gives up encoder work one step a
  second, at the same picture rate: sharp in as few streams as the size allows, then a stream per
  encode engine (bands encoded at once), then the fast preset, and 4:2:0 last, only after three
  seconds still behind at fast and only when the active viewer's chroma is auto. It steps back
  after a while well clear of the picture's time. When a watching viewer's decode of a picture
  (`Stats.decode_us`) takes 90% of a picture's time or more, the size streams in more H.265
  bands (enough for each to decode in 70% of it, at most twice as many a step and 8 in all; the
  viewer decodes the bands at once), and the rate stays: that second's backlog is the decoder's.
  A step that does not cut the decode time by a fifth goes back and is the last. The bands stay
  until a viewer leaves. `Rate` tells the viewers each change.
- **Backpressure.** The desktop never drops an encoded picture of a stream a viewer follows: the
  next one references it. While a watching viewer has more than 2 pictures unsent and no other
  watching viewer keeps up, it makes none; the newest picture goes out once the viewer has caught
  up. A viewer behind while another keeps up is let go of the stream instead: it gets no pictures
  until it has caught up, then starts again at a key frame (asked at most every 2 s per viewer),
  so each viewer gets pictures at the pace of its own link and a slow one never holds the others
  back (`server::Feed`). The adaptive rate follows the viewers that keep up.

## Streams (tiles and interleaving)

**H.265** is one stream as large as the encoder takes (NVIDIA: 8192 wide; no seams, one decoder)
while one hardware decoder keeps up: at most 1.3 Gpixel a second per stream (an Apple M4 decodes
7680x2160 4:4:4 in 7.9 ms alone, in two bands at once in 6.7 ms). Larger is cut into bands from the first picture (7680x2160 at 120:
two bands; 7680x4320 at 120: four), more when a viewer's measured decode asks (above), and when
one engine cannot keep up (the effort steps above) at least one band per encode engine, encoded
at once. Bands are whole macroblock rows, at least 64 rows, and carry `parts`, `x0`, `y0` as tiles do.

A desktop whose GPU has several hardware encode engines encodes one H.264 stream per engine;
each stream is a stream of its own (its own SPS and PPS, its own decoder on the viewer):

- **Tiles** (the default: the lowest latency). Each picture is cut into `columns x bands` tiles
  encoded at once, the engines in turn: the fewest tiles that give every engine the same share,
  that fit the encoder's largest picture (NVIDIA's H.264: 4096 wide, so a 7680-wide desktop has
  two columns or more) and that a decoder takes at the stream rate (H.264 level 5.2: 2,073,600
  macroblocks a second per stream); `tiles = CxR` in the desktop's settings picks them. Cuts are
  whole 16-pixel macroblocks except the last column and band, and cover the picture exactly once.
  Every part of picture `frame` carries `parts` = the tile count and its tile's first column
  `x0` and row `y0`; the tile's size is its stream's SPS size. Tiles of two pictures may be in
  flight at once on the desktop; each picture's parts are still sent back to back. The parts of one
  picture are sent back to back, and the viewer shows a picture only once it has all its parts
  (never a torn picture).
- **Interleave** (the most pictures per second, at one picture's latency). The engines take whole
  pictures in turn: picture `frame` comes from stream `frame % streams` with `parts = 1, x0 = y0 = 0`
  (a picture wider than the encoder takes is cut into columns instead: tiles).
- The desktop's settings pick the split (`split = tiles | interleave | off`).
- A key frame request makes every stream start again from a key frame.
- One engine (or `split = off`): one stream, `stream 0, parts 1, x0 0, y0 0` (or columns, as above).

## Cursor

The cursor is not in the video. The desktop sends `Cursor` whenever the pointer's position over
the streamed screen or its shape changes, apart from the pictures, so it never waits for one.
`shape` indexes this table; an unknown index means the default arrow:

```
0 Hidden  1 Default  2 Crosshair  3 Hand  4 Arrow  5 Move  6 Text  7 Wait  8 Help  9 NotAllowed
10 Grab  11 Grabbing  12 Progress  13 ContextMenu  14 Cell  15 VerticalText  16 Alias  17 Copy
18 NoDrop  19 AllScroll  20 ZoomIn  21 ZoomOut  22 NResize  23 NeResize  24 EResize  25 SeResize
26 SResize  27 SwResize  28 WResize  29 NwResize  30 NsResize  31 NeswResize  32 EwResize
33 NwseResize  34 ColResize  35 RowResize
```

The viewer gives its own OS cursor that shape while its pointer is over the picture, so the local
pointer has no added latency. It draws a cursor sprite at the desktop's position only when the
pointer is moved on the other side.

## Input

The desktop injects input as local devices (Linux: uinput), for the session's life:

- an absolute pointer over the whole desktop, with five buttons and both wheels;
- a keyboard;
- one gamepad per viewer slot.

Its own input stack treats them like plugged-in hardware: key chords, modifiers, focus routing to
hosted apps and Wayland clients, and gamepads in every app. Every held key and button is released
when the session ends (the server does it for every desktop, `server::HeldInput`), and a viewer
silent for 6 s (it pings every second) ends its session. The viewer itself sends the up of every
key and button a window holds whenever that window stops driving the desktop: focus lost, app
inactive, tab or screen changed, history/scrubber, fullscreen change, window closed, quit
(modifiers last). An up is only sent, and only passed on, after its down. The wheel is passed as whole notches; parts of a notch carry over to the
next scroll.

## Scroll direction

Defined once, end to end, so nobody flips it again:

- **makepad `Event::Scroll`**: `scroll.y > 0` = the view moves down the content (the content
  moves up), `scroll.x > 0` = the view moves right. Each platform backend delivers it with its
  OS's direction setting already applied (macOS: the user's natural-scrolling setting; Windows
  and stock Linux desktops: theirs, unchanged).
- **Our Linux WM** (the HAL direct backend) is the one place makepad decides the direction
  itself, per device class: *Natural scrolling* for mouse wheels and for touchpads (Display >
  Mouse & touchpad, both default on = the Mac direction; `direct/preferences.rs`, applied in
  `direct/raw_input.rs`). Every app under the WM, hosted or Wayland, gets the result; no app and
  no WM layer flips anything.
- **The wire** (`Scroll`) carries what the viewer's user saw: `dy = -scroll.y * 3`, `dx =
  scroll.x * 3` (1/120 notches, `makepad_desk_link::scroll_to_wire`). The remote content moves
  the way local content would on the viewer's machine, whatever the desktop's own setting.
- **Desktops inject** so that holds: the WM as a high-resolution wheel on its uinput *Desk Link
  pointer*, the values unturned in evdev's own sense (+1 = view up, tilted right = view right).
  Raw input knows that device by its id (`BUS_VIRTUAL`, `DESK_LINK_INPUT_VENDOR`,
  `DESK_LINK_POINTER_PRODUCT`) and never applies the natural setting to its wheel. Windows
  `SendInput` wheel (+ = view up; Windows' setting only touches precision touchpads, not
  injected wheels); macOS a pixel `CGEvent` scroll (+ = view up; posted events are not turned by
  the user's setting).

## Latency

All server times are on the desktop's clock. The viewer keeps an offset from its pings, taken from
the smallest round trip of the session, NTP-style:

`offset = server_ns − (client_ns + rtt / 2)`

The figures, for each picture shown:

| figure | measured as |
|---|---|
| encode | `encoded_ns − built_ns`: from the start of the desktop's frame build to the bitstream being ready (render + NV12 pack + hardware encode + readout) |
| network | `received − (sent_ns − offset)`: from the desktop's socket write to the viewer's read |
| decode | the hardware decoder's time for the picture |
| present | from decode end to the frame that draws the picture |
| total | the sum: from the desktop's frame build to the viewer's frame |

The viewer shows these in its overlay and sends `Stats` once a second.

## Sessions

A viewer keeps every desktop in its list connected, one connection each whatever number of its
windows show it. Those no window shows are paused: no pictures, and a ping each second keeps
their latency current. Every shown desktop streams at once; a connection is never paused or
closed because another one is shown. Showing a desktop no window showed is `Resume`, answered
with a key frame: one round trip plus one encode, no new TLS handshake.

The desktop serves several sessions at once (8, at most 4 from one address); a new viewer never
ends an existing one. They watch the same screen from one capture and one encode, and the stream
follows the last `SelectScreen`. **Last connection wins**: a desktop that follows its viewer
takes the `Window` of the viewer that connected last (not view-only, its window said); the
earlier ones see that size scaled. When it leaves, the one that connected before it decides
again (`server::size_owner`).

## Semantic inspection (version 8)

Version 8 adds viewer message **25**, `A11yQuery { req u64, line str }`, and desktop
message **109**, `A11yReply { req u64, text str }`. The text is the ordinary `ui`
interpreter reply (including JSON when requested); request IDs correlate concurrent callers.
Only `ui` commands are accepted. The desktop runs them on reusable session workers so a wait
cannot hold up pings, video, input, or the semantic action that satisfies it. Workers
grow to the session's observed query concurrency and stop when it disconnects. The same TLS fleet proof and `Need::Admin` check
that authorizes input injection guards every query, including `ui act` and `ui type`.
Hardware input availability (`Welcome.input`) is independent of this authorization.

Agents address the viewer with `ui machine=TAB ...`, where TAB is the desktop's name
or zero-based tab index. `ui machine=list` lists the available tab indices and names.
The selector is removed before sending the command. For a WM desktop, use
`ui machine=TAB app=CLIENT_ID ...` (or the unique app name); unqualified `ui` includes
the shell and connected hosted trees. `app=shell` selects the WM's local HAL sessions,
including modules and domains. Actions and snapshots should name their app; `w=`
then selects a window within it. `if_user_seq` is checked on the selected desktop/app,
not against the viewer's activity counter.

A disconnected or timed-out query reports an error; actions are never replayed on reconnect.
Version 7 peers are refused during Hello. Live tree embedding in the viewer's own OS
accessibility tree is not part of the query/reply transport.
