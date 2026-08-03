# Makepad Arcade

Networked AI game sandbox (plan: repo-root game.md). Run with:

    cargo run -p makepad-arcade

## Stock assets

Arcade ships with a searchable library of CC0 models and sounds. The binaries
are **not** vendored in git — fetch them once:

    ./apps/arcade/download_assets.sh            # ~19 MB, idempotent
    ./apps/arcade/download_assets.sh --list     # show packs, don't download

That lands 75 models in `resources/models/kenney/`, 556 sounds in
`resources/audio/kenney/`, and the rigged character in `resources/characters/`
— all gitignored, all pinned to exact upstream commits (models) or
content-hashed URLs (audio) and verified by sha256, so a moved or tampered
upstream fails loudly instead of silently changing the library.

Everything degrades gracefully without them: the demo runs with primitive
shapes, and tests that need real assets skip with a hint.

**Audio is Ogg Vorbis.** Every Kenney audio pack ships `.ogg` only — no WAV
variant exists upstream — and this tree has no vorbis decoder. The sounds are
therefore indexed and searchable by the AI but **not yet playable**; adding a
decoder (or running `download_assets.sh --transcode`, which converts to WAV
when ffmpeg is installed) is what closes that gap.

### Finding assets

The library is queried by *description*, never by filename, via
`makepad-game-assets`. The agent gets a `find_model` tool and a one-paragraph
summary of the library — it searches, it never receives the catalogue. Ids look
like `kenney/racing/vehicle-truck-yellow` and are stable across re-downloads,
because generated game code writes them.

## Credits

Every asset here is CC0 (public domain). Attribution isn't required by the
licence — we credit anyway, because these libraries exist because someone chose
to give them away.

- **Kenney** — <https://kenney.nl/assets> — the arena, city, FPS, platformer and
  racing model kits, and all seven sound packs (impact, interface, sci-fi,
  music jingles, UI, RPG and digital audio). Thank you for the extraordinary
  breadth of free, consistent, genuinely usable game assets.
- **KayKit / Kay Lousberg** — <https://kaylousberg.itch.io/> — the rigged and
  animated adventurer characters. Thank you for giving away rigs and animation
  sets, which are the expensive part.

Machine-readable attribution lives in `resources/CREDITS.toml` so a published
game package can carry credit with it.
