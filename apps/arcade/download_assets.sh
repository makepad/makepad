#!/usr/bin/env bash
# Fetch the CC0 character assets Makepad Arcade uses for its stock skinned
# characters. Files are pinned to an exact upstream commit and verified by
# sha256 — the repo never vendors the binaries (see resources/characters/
# .gitignore); run this once after checkout.
set -euo pipefail

DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/resources/characters"
mkdir -p "$DIR"

# KayKit Character Pack : Adventurers (CC0) — pinned commit.
COMMIT="672074b73ba276876a19e8816ecdc5241817ab47"
BASE="https://raw.githubusercontent.com/KayKit-Game-Assets/KayKit-Character-Pack-Adventures-1.0/$COMMIT"
CHARS="addons/kaykit_character_pack_adventures/Characters/gltf"

fetch() { # <relative-url> <dest-name> <sha256>
	local url="$BASE/$1" dest="$DIR/$2" sha="$3"
	if [[ -f "$dest" ]] && echo "$sha  $dest" | shasum -a 256 -c - >/dev/null 2>&1; then
		echo "ok (cached): $2"
		return 0
	fi
	echo "fetching: $2"
	curl -sSfL "$url" -o "$dest.tmp"
	if ! echo "$sha  $dest.tmp" | shasum -a 256 -c - >/dev/null 2>&1; then
		echo "ERROR: sha256 mismatch for $2 (upstream changed or download corrupted)" >&2
		echo "  expected: $sha" >&2
		echo "  got:      $(shasum -a 256 "$dest.tmp" | awk '{print $1}')" >&2
		rm -f "$dest.tmp"
		exit 1
	fi
	mv "$dest.tmp" "$dest"
	echo "ok (fetched): $2"
}

fetch "$CHARS/Knight.glb" knight.glb \
	60428e3abc09ba83e595d256e3af8c5c976b46cdae599f0802fc82b4a3445168
fetch "$CHARS/knight_texture.png" knight_texture.png \
	5d250ccc5da020e6126bfa3839f83bd9a465a951ed223e4d13c08b1925e154d4

echo "arcade assets ready in $DIR"
