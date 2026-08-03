# Makepad Arcade

Networked AI game sandbox (plan: repo-root game.md). Run with:

    cargo run -p makepad-arcade

## Stock assets

Arcade ships with a searchable library of CC0 models and sounds. The binaries
are **not** vendored in git — fetch them once:

    ./apps/arcade/download_assets.sh            # core packs, ~75 MB
    ./apps/arcade/download_assets.sh --packs=all   # everything, ~185 MB
    ./apps/arcade/download_assets.sh --list        # show packs, download nothing

The default fetches a **core** set (~1900 models) so a fresh clone isn't forced
to pull the lot; `--packs=all` gets the full Kenney 3D catalogue (**4669 models
across 47 packs**), and `--packs=nature-kit,car-kit` picks specific ones. Add
the 556 sounds and the rigged KayKit character and the full library is ~5000
searchable assets.

Everything is gitignored and pinned — starter kits to exact GitHub commits, the
rest to content-hashed kenney.nl URLs — and every file is sha256-verified, so a
moved or tampered upstream fails loudly instead of silently changing the
library. Downloads are sequential with a delay: this is someone else's
bandwidth.

### Mirroring

`resources/MIRROR.toml` lists every pack with its canonical URL, sha256, size
and model count. Because these assets are CC0, anyone may re-host them, and
that file is what makes a mirror reproducible and verifiable. Point at one with:

    ARCADE_ASSET_MIRROR=https://your.host/assets ./apps/arcade/download_assets.sh
    ./apps/arcade/download_assets.sh --mirror=https://your.host/assets

A mirror is expected to serve `<base>/<slug>.zip`. **The sha256 from
MIRROR.toml is verified identically whichever host served the bytes** — a
mirror is never trusted more than upstream; the hash is the authority.

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
summary — it searches, it never receives the catalogue (5000 entries would not
fit in a prompt, and the summary stays ~480 characters however large the
library grows). Ids look like `kenney/racing/vehicle-truck-yellow` and are
stable across re-downloads, because generated game code writes them.

Findability is built in three layers so it scales past 4000 models:
per-pack theme curation (~55 rows, giving every model its setting), filename
token parsing (free, and Kenney's names are systematic), and a hand-curated
query-time synonym table (~240 rows) that applies to the whole catalogue at
once. Item-level curation is spent only on the few hundred most-requested
things. At the full 4,999-entry catalogue: index build ~120 ms, a search
~0.2 ms, ~2.1 MB of heap (release).

Query-side stemming means an inflected request still reaches a base-form
alias ("smashing" → the `smash` alias), and when two entries tie on score the
kind the query implies wins — an unqualified noun like "spaceship" is an
object request, while "metal clang" or "win music" wants something audible.

## Credits

Every asset here is CC0 (public domain). Attribution isn't required by the
licence — we credit anyway, because these libraries exist because someone chose
to give them away.

- **Kenney** — <https://kenney.nl/assets> — 47 model packs totalling ~4,670
  models (nature, city, castle, space, food, furniture, vehicles, dungeons,
  characters and more), the five starter kits, and all seven sound packs
  (impact, interface, sci-fi, music jingles, UI, RPG and digital audio). Thank
  you for the extraordinary breadth of free, consistent, genuinely usable game
  assets — this library is most of what Arcade can build with.
- **KayKit / Kay Lousberg** — <https://kaylousberg.itch.io/> — the rigged and
  animated adventurer characters. Thank you for giving away rigs and animation
  sets, which are the expensive part.

Machine-readable attribution lives in `resources/CREDITS.toml` so a published
game package can carry credit with it.
