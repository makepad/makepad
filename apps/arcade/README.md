# Makepad Arcade

Networked AI game sandbox (plan: repo-root game.md). Run with:

    cargo run -p makepad-arcade

Stock character assets (KayKit Adventurers, CC0 — see
resources/characters/LICENSE-CC0.txt) are not vendored in git; fetch them
once with:

    ./apps/arcade/download_assets.sh

Everything degrades gracefully without them (the demo just has no skinned
character and tests that need the real model skip with a hint).
