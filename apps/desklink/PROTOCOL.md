# Desk Link protocol, version 6

Desk Link is how a viewer (this app) sees and drives a desktop (the window manager's
`desk_link` server, `apps/wm/src/desk_link/`). It runs between machines on one LAN only: no
relays, no NAT traversal, no discovery service. The one implementation of the format is
`libs/desk_link`, used by both ends.

## Transport and security

- TCP, default port **8790**, wrapped in TLS on each OS's own TLS stack (`makepad_network::tls`).
- The desktop presents a self-signed certificate (`~/.makepad/wm/desk-link/tls`). The viewer
  pins it ssh-style in `~/.makepad/ai-hub/known_nodes` (`host:port`). A changed certificate is
  reported loudly and the connection goes on: the fleet proof below is what protects the desktop.
- The desktop serves only when **all** of these hold. Otherwise nothing listens:
  - its settings enable it (`~/.makepad/wm/desk-link.conf`, `enabled = on`);
  - the bind address is loopback, a private LAN address or `0.0.0.0`;
  - its fleet key (`fleet.key`, the AI hub's `<cache>/fleet`) loads.
- **Peer allowlist.** Each server serves only the addresses in its `allow_peers` setting (the
  viewer machines). The list is required: an empty or missing list means the server does not
  start, so it never runs open to the whole LAN on a credential alone.
  - Anything else is closed before TLS, and the address is checked again after authentication.
    The peer must also be a LAN address (loopback, RFC 1918, link-local, IPv6 ULA).
  - Refusals are logged with the address only, at most once a minute per address.
  - Five failed credentials within a minute ban the address for 15 minutes. Sessions are capped
    at 8, with at most 4 per address.
  - IP pinning assumes the viewer machines have a DHCP reservation, so their address does not
    change.
- **Authentication.** The viewer's first message carries a fleet proof:
  `MKC2 <id>.<role>.<expiry>.<HMAC-SHA256(secret, "mkfleet2 proof|" + certificate sha256 hex)>`.
  - The secret comes from the viewer's credential (`~/.makepad/ai-hub/client.credential`).
  - The proof is bound to the certificate the desktop presented, so a proof captured by anything
    else is useless.
  - The desktop requires role `lan`, an unexpired credential, and an id not on its `revoked` list.
  - Anything else is answered `Refused` and closed.

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
| 1 | Hello | `version u16, name str, authorization str, screen i32 (-1 = the desktop's default), paused bool, caps u32` (1 = the viewer's decoder takes H.264 High 4:4:4 Predictive; 2 = view-only: its `Window` never decides the size of a desktop that follows its viewer, e.g. a probe) |
| 2 | Pause | — no pictures for this session; pings continue |
| 3 | Resume | — pictures again, starting with a key frame |
| 4 | SelectScreen | `index u32` (into `Welcome.screens`) |
| 5 | RequestKeyframe | — (sent by the viewer after a decode error) |
| 6 | SetSplit | `split u8` (0 one engine, 1 tiles, 2 interleave: how a desktop with several encode engines shares them, for every viewer) |
| 7 | Refresh | `hz u16`: the viewer's display refresh, measured from its vsync (0 = not known). Sent after Hello and on every change. |
| 8 | Window | `width u32, height u32, scale u16`: the viewer's picture area in device pixels and its scale (hundredths, 200 = 2.00x). Sent once it has held still for 250 ms, after the Welcome and after every resize. A desktop whose size follows its viewer (a virtual display set to follow) takes this size and scale from the active viewer (the watching viewer that sent input last), so the picture is shown 1:1. |
| 10 | Key | `down bool, keycode u32 (makepad KeyCode index), mods u8 (1 shift, 2 control, 4 alt, 8 logo)` |
| 11 | Pointer | `x u16, y u16`: position over the stream picture, 0..=65535 on each axis |
| 12 | Button | `down bool, button u8 (0 left, 1 right, 2 middle, 3 back, 4 forward)` |
| 13 | Scroll | `dx i32, dy i32` in 1/120 notches; +dy = wheel away from the user |
| 14 | Gamepad | `slot u8, buttons u32, left_x i16, left_y i16, right_x i16, right_y i16, left_trigger u8, right_trigger u8` |
| 15 | GamepadGone | `slot u8` |
| 20 | Ping | `client_ns u64` (the viewer's clock) |
| 21 | Stats | `frame u32, decode_us u32, present_us u32` |
| 22 | Clipboard | `text str` (the desktop's clipboard takes it) |
| 23 | RateCap | `mbps u16`: the most this viewer wants the stream to take (all tiles; 0 = no cap of its own). The desktop streams at most the lowest cap of its watching viewers. |

Gamepad buttons: bit 0 A, 1 B, 2 X, 3 Y, 4 left shoulder, 5 right shoulder, 6 select, 7 start, 8 home,
9 left thumb, 10 right thumb, 11-14 d-pad up/down/left/right. Sticks run −32767..32767, up positive
(makepad's convention); triggers 0..255.

## Desktop -> viewer

| type | name | payload |
|---|---|---|
| 100 | Welcome | `version u16, session u32, screens (n u32, then per screen: name str, x u32, y u32, width u32, height u32), screen u32, stream_w u32, stream_h u32, input bool, input_note str, server str` |
| 101 | Refused | `reason str` (then the desktop closes) |
| 102 | Video | `frame u32, key bool, built_ns u64, encoded_ns u64, sent_ns u64, stream u8, parts u8, x0 u32, y0 u32, data bytes` (see Streams) |
| 103 | Cursor | `x u16, y u16 (over the stream picture), shape u8, visible bool` |
| 104 | Pong | `client_ns u64 (echoed), server_ns u64` |
| 105 | Stream | `screen u32, stream_w u32, stream_h u32` (the stream shows another screen now) |
| 106 | Clipboard | `text str` |
| 107 | Bye | `reason str` (then the desktop closes) |
| 108 | Rate | `fps u16, kbps u32, limit u8, full_chroma bool`: the pictures a second the desktop streams at now (the lowest of its settings' cap (`fps`, default 120), its streamed screen's refresh and every watching viewer's `Refresh`; a still desktop sends fewer), the stream's rate (kbit/s, all tiles; 0 = by picture size), what holds the rate (0 starting, 1 nothing: rising, 2 the link, 3 the encoder, 4 a cap, 5 a fixed setting) and whether it is 4:4:4. Sent after Welcome and on every change. |

Screens are in desktop device pixels. `input = false` means viewer input is not injected (for
example, no `/dev/uinput` access); `input_note` says why.

## Video

- **Codec.** H.264 High, 4:2:0 8-bit, or High 4:4:4 Predictive (4:4:4 8-bit, CAVLC) when the
  desktop's settings ask for it (`chroma = 444`, the default) and every watching viewer's Hello
  says it decodes it; BT.709 limited range (written in the SPS VUI), Annex-B access units (one
  picture each).
  - Every key frame (IDR) starts with its SPS and PPS.
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
  `kbps` setting, or adaptive: it starts at 250 Mbit/s (or the size's own rate, about 0.45 bit per
  pixel per picture at 4:4:4, if lower), goes down below what got out when a viewer falls behind
  (link-bound), and creeps back up when clean, up to the size's rate, `max_mbps` and
  the viewers' `RateCap`. When the encoder cannot keep a picture's time at 4:4:4 (its slowest
  tile takes most of it and frames wait for it), that size streams 4:2:0 at the same picture
  rate, and tries 4:4:4 again after a while. `Rate` tells the viewers each change.
- **Backpressure.** The desktop never drops an encoded picture: the next one references it. While
  a watching viewer has more than 2 pictures unsent, it makes none. The newest picture goes out
  once the viewer has caught up.

## Streams (tiles and interleaving)

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
when the session ends. The wheel is passed as whole notches; parts of a notch carry over to the
next scroll.

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

A viewer keeps every desktop in its list connected. Those it does not show are paused: no
pictures, and a ping each second keeps their latency current. Showing another desktop is
`Pause` to the old one and `Resume` to the new one, which answers with a key frame: one round
trip plus one encode. It needs no new TLS handshake.

The desktop serves any number of sessions. They watch the same screen, and the stream follows
the last `SelectScreen`.
