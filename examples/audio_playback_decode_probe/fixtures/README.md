# Audio decode probe fixtures

Optional fixtures for the Month 2 audio decoder support probe:

- `mono_100ms_44100.mp3`: 100 ms, 44100 Hz, mono MP3.
- `stereo_100ms_48000.opus.ogg`: 100 ms, 48000 Hz, stereo OGG/Opus.

When both files exist and are non-empty, `build.rs` enables the fixture decode
probe automatically. The example still compiles without them and reports that
valid decode fixtures are missing.
