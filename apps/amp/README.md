# Amp

Makepad Amp is one music player for the music systems installed on the
machine, drawn as a piece of hardware. Each system is a `MusicSource`
(`src/source`): Spotify through the command line its desktop app ships, the
Spotify desktop app's own player, D-Bus players on Linux, and local files
(mp3, ogg, flac) that Amp plays itself.

- A 20-band graphic equalizer and output level on the sound the player
  makes (Amp taps the player's output on macOS; macOS asks once).
- The screen shows a spectrum or the Visualizer: presets rendered from the
  sound (`libs/milkdrop`). Amp ships a few presets of its own
  (`resources/visualizer`); the preset browser downloads a larger pack on
  request. A tap on the picture shows it alone, a sideways swipe changes
  the preset, and a pop-out window shows it edge to edge. On a phone the
  picture renders at a fixed pixel budget, scaled up to the screen.
- With no music system to connect to (a phone, Spotify not running) Amp
  opens on the cached songs, and Play plays them.
- Styles (`resources/styles`) and layouts (`resources/layouts`) restyle and
  rearrange the device; the AI stylist writes styles and visualizer presets
  with your own Claude Code or Codex.
- A queue, folders, playlists and search over every connected system.

```sh
cargo run --release -p makepad-app-amp
cargo run -p makepad-run --release -- run makepad-app-amp   # makepad run (JIT)
```

Data lives in `local/amp/` inside the Makepad checkout (an Amp installed
outside a checkout uses the app's data folder). The portable offline
library is `local/amp/offline/` (`songdb.json`, `songs/`, `art/`): Amp shows
and plays those songs without a Spotify connection, and copying the folder
to another checkout carries the songs, metadata and artwork along.
`AMP_DATA_DIR` overrides the data folder for an isolated run. Environment:
`MUSIC_MUTE=1` processes the sound without playing it (test runs).

Credits and third-party notes: [CREDITS.md](CREDITS.md).
