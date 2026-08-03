#!/usr/bin/env bash
# Fetch the CC0 model packs Makepad Arcade uses for its stock asset library.
# Nothing here is vendored in git (see resources/*/.gitignore) — run this once
# after checkout. Every file is pinned to an exact upstream commit and verified
# by sha256, so a moved or tampered upstream fails loudly instead of silently
# changing the library.
#
#   ./download_assets.sh          fetch everything (idempotent)
#   ./download_assets.sh --list   show packs, counts and licences, download nothing
#
# ----------------------------------------------------------------------------
# CREDITS — all assets below are CC0 (public domain). Attribution is not
# required by the licence; we credit anyway, because these libraries exist
# because someone chose to give them away.
#
#   Kenney — https://kenney.nl/assets
#     Vehicles, city, arena, platformer and FPS kits. Thank you @KenneyNL.
#
#   KayKit / Kay Lousberg — https://kaylousberg.itch.io/
#     Rigged + animated adventurer characters.
#
# NOTE ON AUDIO FORMAT: every Kenney audio pack ships Ogg Vorbis only (no WAV
# variant exists upstream — checked all seven packs). This tree has no vorbis
# decoder, so the sounds are downloaded and indexed (searchable by the AI) but
# not yet playable; a decoder, or an opt-in local transcode, is the missing
# piece. `--transcode` below does the latter when ffmpeg is installed.
# ----------------------------------------------------------------------------
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/resources"
MODELS="$ROOT/models/kenney"
CHARS="$ROOT/characters"
AUDIO="$ROOT/audio/kenney"
MANIFEST="$ROOT/MIRROR.toml"

# ---------------------------------------------------------------------------
# 3D model packs (the full Kenney 3D catalogue) are described by
# resources/MIRROR.toml rather than inlined here: 50 packs is too many to keep
# readable in shell, and that file is also what makes an independent mirror
# reproducible.
#
# Source selection:
#   ARCADE_ASSET_MIRROR=https://our.host/path   or  --mirror=<base>
# A mirror is expected to serve <base>/<slug>.zip. Whichever host serves the
# bytes, the sha256 from MIRROR.toml is verified identically — a mirror is
# never trusted more than upstream.
#
# Selection:
#   (default)        the "core" packs — ~1900 models, ~60 MB
#   --packs=all      every usable pack — 4669 models, ~166 MB
#   --packs=a,b,c    named packs
# ---------------------------------------------------------------------------
MIRROR="${ARCADE_ASSET_MIRROR:-}"
PACKSEL="core"
fetched=0
cached=0


# Kenney audio packs, pinned by content-hashed download URL.
# Each entry: <pack>|<url-hash-segment>|<sha256 of zip>|<sound count>
KENNEY_AUDIO=(
	"impact-sounds|87b4ddecda-1677589768|029d734af1582474edf3a694d1b0cebc97c1c152f2f39fa34d4c2bafc5de77f8|130"
	"interface-sounds|fa43c1dd4d-1677589452|f2193d072726d6758a5f7871b2dcc54dcce0d5c35c6f0a62f92549b327c81232|100"
	"sci-fi-sounds|6b296f9ecf-1677589334|119340f351a5098ad814f78719438c0da355a9ce8a4c8a3af6a8d48aa3d49e04|73"
	"music-jingles|f37e530b9e-1677590399|b729ba57959bd58793d2c5cafa348aaf2655d354f3da35ec4729e03ec77197b8|86"
	"ui-audio|490d233f68-1677590494|946fc23a63d535d693eb31b2eabb80c8c28d6351e2186b344ceb71b2cb1d5eb6|52"
	"rpg-audio|8e99002d76-1677590336|6dbeaf8544da958d8f2adcb4a4a4b76c1ade34a05f8ab9edccd327da7375f38b|52"
	"digital-audio|216eac4753-1677590265|24e6ce28b76a6d8c89cff4d331e0965ff5c3de8a73c612028e9d363cc64e4f06|63"
)

# Kenney starter kits, pinned. Each entry: <pack> <repo> <commit> <subdir>
KENNEY_PACKS=(
	"arena|Starter-Kit-Basic-Scene|a6927e66ff8dd8e173660ce4825abe773c65f683|sample/Mini Arena/Models/GLB format"
	"city|Starter-Kit-City-Builder|4535092b740b378b700efd9df9e27a631815b84a|models"
	"fps|Starter-Kit-FPS|185fd2326d74a5cf858cffc616f87cf9696f9cc0|models"
	"platformer|Starter-Kit-3D-Platformer|3fa8a04b1c01ab23db43123d4ce814a34c3fc7f0|models"
	"racing|Starter-Kit-Racing|f5241ebdf00c25bc951bf4fdb7950bb1b78b4bcc|models"
)

# KayKit Character Pack : Adventurers, pinned.
KAYKIT_COMMIT="672074b73ba276876a19e8816ecdc5241817ab47"
KAYKIT_BASE="https://raw.githubusercontent.com/KayKit-Game-Assets/KayKit-Character-Pack-Adventures-1.0/$KAYKIT_COMMIT"
KAYKIT_DIR="addons/kaykit_character_pack_adventures/Characters/gltf"

# Per-pack file manifest: "<filename> <sha256>" lines.
pack_files() {
	case "$1" in
	arena)
		echo "banner.glb 96e1cf7924fcc871be0e7e182c9b918de381b9783ca9e96922f8db62af384056"
		echo "block.glb 656e07f0a3e7d0620892aca6fc40683b4266ff6b9672ecf89f3553e5ed85a1fb"
		echo "border-corner.glb 120ce330ca50fe50e51bf693eadb957abaca972c42d573a07d8e8e404560bc3c"
		echo "border-straight.glb 76ef69e8858f3d17c652d92033a553db58296135a5810d1bd5e54f47b5aa40c4"
		echo "bricks.glb fd1d2c033c7db8b7352ac1becdab447d09cabcc11c73adf9bb6790f9680eaa86"
		echo "character-soldier.glb b0410b4a34068e56ac1af3a2571f4497f84f1e481cced895b618d1fe8136529c"
		echo "column-damaged.glb 442a6856e7e2d042c9e3af155dabf6d394f7ba91b2e271ee6392da22555a072c"
		echo "column.glb 5c815b5fd43d613a21afddaaa90dc2f90b80e31f03dbdde98921cb7424e836c0"
		echo "floor-detail.glb 804224caf3008fcdbd57d7ccc2c4734320c58ca880895699307670d2443e324b"
		echo "floor.glb 15512c5ca6e9d3eb60efd237bbbf5b8010fdea63e0d8e6623f72b59de20279ab"
		echo "stairs-corner-inner.glb 2e232cecda1d633acc1b78707ee50089fd40a167293c173536d61ef064697a5c"
		echo "stairs-corner.glb af8eb87dc8826e184f585ebccf0592c909da7370559cd840b218593f4b1c8a38"
		echo "stairs.glb 54a3f12c720f9cb83027b5429e330d7c50578f70771fd1bf9a2e521fe131ca6f"
		echo "statue.glb 5ba8ecba963f6b3ec974809e2bf0b615da7e2701cd0efc6a3c0e946efb147397"
		echo "tree.glb f8632c9758a78f60aa4ca68bf0e5ac972770167a5608e179293a12552d8fc2ef"
		echo "trophy.glb 253f6d5f321ab20f8f505dfe35b7404c98d2d492728e6df5f69dd29e936b8be5"
		echo "wall-corner.glb 4dc0e770106a122885f41be0bb4024466a7f58bc29a13f07b6600c4babbeb777"
		echo "wall-gate.glb f70a86344f202b3a8057e3a6ee9cd632ba51a5c16795cfde34cd10e906f14697"
		echo "wall.glb b05ab8645136cc26e761ae2189f660a585dca88c11721771bca3fb4c1f7a807f"
		echo "weapon-rack.glb bb95da3746098e36f3d87a1e1036ca70ead7fe95860191871e02f7c58ff1efe5"
		echo "weapon-spear.glb 9634e12abce01a07d40d5156454bfa1123eba72af6449fccf577980e331adbeb"
		echo "weapon-sword.glb e4d2219d954148dd8b04fccc7602726c34eafad58c0dc5a45268155fcf6cc8c4"
		;;
	city)
		echo "building-garage.glb 7373b558fc9b1e27e60b60ca1ce53000af7e132d7304eeb6d49b47528a3b6806"
		echo "building-small-a.glb 22ce989013bd16b1732e81798e343cf85f947e92b6c93825b18131feded48e07"
		echo "building-small-b.glb 0bc8459045975158524753d6b648ff77e5f91fe8c033bc4886930b6d55543a2b"
		echo "building-small-c.glb 3ea0f46fbed4e0acba7fda365cd968b3f23a6e02b9fee79dd54c99c42ffadf2f"
		echo "building-small-d.glb 0ab476fbd7956cea52e590a8e68e351ce2f6fc04df158aa3f5a76b7744bf37a0"
		echo "grass-trees-tall.glb d23e3b722453236bf6f4f8922b7bf64f81a7f3f804848a89c4b100ccbf13abef"
		echo "grass-trees.glb cb06d03cc1c64ca7e7692c0aef3628d8e7cd4afee69093025f9ddd668653a4ce"
		echo "grass.glb 3e3ec91132ad8519967aa2e4c0bedbab2aeed39b5714d66068bb75648ae73a52"
		echo "pavement-fountain.glb ea2996089e90a79ba13d764106d374486bb8137666cf1cdbaebd1927ef365185"
		echo "pavement.glb 60152776325d436761a13eb0c2ec9368b393bc82cf754b63cab810d79639ade9"
		echo "road-corner.glb 85aec60d66c5084bb658274a1468139a5def1e9e9575c01286ed6c2069916789"
		echo "road-intersection.glb 0212ffe9945b933535503372855c54dee20d54ddfc07e1c72963e67558a14183"
		echo "road-split.glb 31bf582953db8315edfade4ba49d81145a78bda9757b6442dc417e14cc84b5ff"
		echo "road-straight-lightposts.glb fc5340621fefdd43ab0bbf4bdaa41de9822dc545ce787fbfd92b75ce85d51e62"
		echo "road-straight.glb 008a6305de778439d1a99be78a3e0945c72a9bc9cc0bade1b51a666d5db01d0b"
		;;
	fps)
		echo "blaster-repeater.glb e76220e5ad3877d879e70fe21bd5bc76a5988b8682aa5f7976739e71caf70e05"
		echo "blaster.glb 9c2110d94c1bd7e01bfe6827a3d87e14962a1f721d8b300c67bdd36f492140a1"
		echo "cloud.glb 4c667239958ae8950b6bc2f55bdb59814b24fc9f6d8531eb09411150e09c87f2"
		echo "enemy-flying.glb e4868f6d8cac2a8f728430229789206841ca64727d94de2d808b468e4f182f8c"
		echo "grass-small.glb 8dfe00761fa833daab3192148b9b3d573f2a10aa2eff2c01a8a386dd304e7fda"
		echo "grass.glb 8e11283ca11894860a4dcb8f523165eda5de39f68fda43d892efc3a3d3eb567e"
		echo "platform-large-grass.glb 19ee4b9e745f757d87d584094433e3f4d86be958bcd7eeba586874f088d2697b"
		echo "platform.glb 6eac72ffbda64b4414833eb49d3b160fddf169c2caccca237a07e15a20731764"
		echo "wall-high.glb 946a0f875bd515323afd883553729a29ac764b405e382bf9a30d6527dac74589"
		echo "wall-low.glb c42c89dead3835fde97d1ede232bf1a69d297cd41e1d6bfe7e25c87b233a2ec0"
		;;
	platformer)
		echo "block-coin.glb ebe3a7be051566513039e6ba7d83d976fb3bf3e363fc8ee0633bdb872bb2d501"
		echo "brick-particle.glb 981a84073e1dfa745a93db4ba550210049a144a4ac2c6cd333f289ff95630a72"
		echo "brick.glb c9011dd30254c7a5dfdb29c61c22b6b3b4ad82186a096b90b824ab0b8dd58db8"
		echo "character.glb 7112f6a08400914f9da546f3e6029e947cc9eab2b4a6da5eb99776111289efb1"
		echo "cloud.glb 09c6f071d3a9ab64993248ee5c8d50debcd9c6cc4369236a01155ff7ff87f30a"
		echo "coin.glb a3fb8f779a6af1cd75e5a02da77a87546ca8a59f6fe52f79f25179c4a68fd0e5"
		echo "dust.glb 2f2ce449aca7791829bf2d93a7af534549f0bad1ee904d1a00df8d5329fc31dc"
		echo "flag.glb 0ba84ff43f0fea0a9f0b080cfa1be54ebda36c8c35174d1ac2553ae6da9f65ae"
		echo "grass-small.glb 4b94e8dd9cbdc3eedff99cbf614fe828f0c2342d21a7d0edb125b129d0422d51"
		echo "grass.glb acb29df8a75b08e985e27a0ed8170b8f3d135abd24ccf5aa4d631cb2cd15cfda"
		echo "platform-falling.glb d6dc9baba80af659e6d0fffc435bd1f08b1a7945942ccbb85dd9bb103b7a574d"
		echo "platform-grass-large-round.glb 73c2b66eca6a36df6b5c25bb2c0594cf870066627275993a3a7857acc5a844bc"
		echo "platform-large.glb d01f4eae24c895dfe467ef71c2ea510ca6bd6d8462f319741537936eae0b3f65"
		echo "platform-medium.glb 64f81c8bd8bf07cc450dfcb25d18eedcc122d690028cca098bcdd48b00a02e46"
		echo "platform.glb 78c5ed4da30c5a97f0747a54248bc9de05ba1b3fb6b94be527dd48599f5ef44c"
		;;
	racing)
		echo "decoration-empty.glb 3815b26a5274173934d37cf605e320450efad4e0034040868b6aea761cfd74bc"
		echo "decoration-forest.glb 664a53f0f709fef9096af3bbfb1aa76536527a616b05170c0cf4e27e33358a00"
		echo "decoration-tents.glb 19dbf2a778ad75f95c7d61f12866ef7174ab69cad169d3749c5400f5e14db8a3"
		echo "track-bump.glb 6db020edc53532ebc0971c9d4ad9afb11c4de67c510649ca843db4616514f43c"
		echo "track-corner.glb 0ffb3d83b60456fc5a2962447111a1a64bfe943f4ad95462e1edba107be81c80"
		echo "track-finish.glb 2fec3b681658d6e77e20c4342d1e3ceeaab3e8d2fbf340942cde25cc2b21975b"
		echo "track-straight.glb 2d8080df1fe27e39981480809f36eba7f239e814f96bff61bb4c99793221b701"
		echo "track-tents.glb eaf68bbb44e362e291b71d13015fbafc4ce22a9102bda029614972d6cc843c7f"
		echo "vehicle-motorcycle.glb c97911b8dbc2d5dd9d1961b46eb4d1f132b67a6c5a84f45593fea44e0595dd92"
		echo "vehicle-truck-green.glb df362d027a09b19395be0e96abd7cc2dc54608dfb920b592e64357b589f9fcd6"
		echo "vehicle-truck-purple.glb 03e32f5abcbbd03da3591f42fdaae2cf7d27ed379df7ce0d1f233bea02998565"
		echo "vehicle-truck-red.glb eca99bd9ab0a2b02125f915e65d1ec8f1c5a93be7c6d4840efdac6633f47772c"
		echo "vehicle-truck-yellow.glb 1ebd83174eab6d2fdf69eb5d8e32bd06a23a86d9edc4fe915a4160430a7c36a5"
		;;
	*)
		echo "unknown pack: $1" >&2
		return 1
		;;
	esac
}

pack_count() { pack_files "$1" | wc -l | tr -d ' '; }

# Read MIRROR.toml into parallel arrays: slug, url, sha, models, core, usable.
manifest_rows() {
	awk '
		/^\[\[pack\]\]/ { slug=url=sha=""; models=0; core="false"; usable="false"; next }
		/^slug =/    { gsub(/.*= "|"/, ""); slug=$0; next }
		/^url =/     { gsub(/.*= "|"/, ""); url=$0; next }
		/^sha256 =/  { gsub(/.*= "|"/, ""); sha=$0; next }
		/^models =/  { gsub(/[^0-9]/, ""); models=$0; next }
		/^core =/    { gsub(/.*= /, ""); core=$0; next }
		/^usable =/  { gsub(/.*= /, ""); usable=$0
		               if (slug != "") print slug "|" url "|" sha "|" models "|" core "|" usable
		               next }
	' "$MANIFEST"
}

want_pack() { # <slug> <core>
	case "$PACKSEL" in
	all) return 0 ;;
	core) [[ "$2" == "true" ]] ;;
	*) [[ ",$PACKSEL," == *",$1,"* ]] ;;
	esac
}

if [[ "${1:-}" == "--list" ]]; then
	echo "Makepad Arcade stock asset library"
	echo
	printf '%-12s %-6s %-10s %s\n' "PACK" "FILES" "LICENCE" "SOURCE"
	for entry in "${KENNEY_PACKS[@]}"; do
		IFS='|' read -r pack repo _commit _dir <<<"$entry"
		printf '%-12s %-6s %-10s %s\n' "$pack" "$(pack_count "$pack")" "CC0-1.0" "kenney.nl (KenneyNL/$repo)"
	done
	printf '%-12s %-6s %-10s %s\n' "characters" "1" "CC0-1.0" "kaylousberg.com (KayKit Adventurers)"

for entry in "${KENNEY_AUDIO[@]}"; do
		IFS='|' read -r pack _hash _sha count <<<"$entry"
		printf '%-12s %-6s %-10s %s\n' "$pack" "$count" "CC0-1.0" "kenney.nl (ogg)"
	done
	if [[ -f "$MANIFEST" ]]; then
		echo
		echo "3D packs (from MIRROR.toml; * = in the default core set):"
		tm=0; tp=0
		while IFS='|' read -r slug _url _sha models core usable; do
			[[ -z "$slug" ]] && continue
			mark=" "; [[ "$core" == "true" ]] && mark="*"
			note=""; [[ "$usable" == "true" ]] || note="  (FBX only — not fetched)"
			printf '%s %-34s %5s models%s\n' "$mark" "$slug" "$models" "$note"
			[[ "$usable" == "true" ]] || continue
			tm=$((tm + models)); tp=$((tp + 1))
		done < <(manifest_rows)
		echo "  ${tp} usable packs, ${tm} models total"
	fi
	echo
	echo "Run without --list to download. Files land in resources/models/kenney/<pack>/,"
	echo "resources/audio/kenney/<pack>/ and resources/characters/ — all gitignored."
	echo
	echo "Audio is Ogg Vorbis (Kenney ships no WAV). This tree has no vorbis decoder,"
	echo "so sounds index and search but do not play yet; --transcode converts them to"
	echo "WAV with ffmpeg if you have it installed."
	exit 0
fi

TRANSCODE=0
for arg in "$@"; do
	case "$arg" in
	--transcode) TRANSCODE=1 ;;
	--packs=*) PACKSEL="${arg#--packs=}" ;;
	--mirror=*) MIRROR="${arg#--mirror=}" ;;
	--list) : ;;
	*) echo "unknown option: $arg" >&2; exit 1 ;;
	esac
done



fetch() { # <url> <dest> <sha256> <label>
	local url="$1" dest="$2" sha="$3" label="$4"
	if [[ -f "$dest" ]] && echo "$sha  $dest" | shasum -a 256 -c - >/dev/null 2>&1; then
		cached=$((cached + 1))
		return 0
	fi
	if ! curl -sSfL "$url" -o "$dest.tmp"; then
		echo "ERROR: download failed: $label" >&2
		echo "  $url" >&2
		rm -f "$dest.tmp"
		exit 1
	fi
	if ! echo "$sha  $dest.tmp" | shasum -a 256 -c - >/dev/null 2>&1; then
		echo "ERROR: sha256 mismatch for $label (upstream changed or download corrupted)" >&2
		echo "  expected: $sha" >&2
		echo "  got:      $(shasum -a 256 "$dest.tmp" | awk '{print $1}')" >&2
		rm -f "$dest.tmp"
		exit 1
	fi
	mv "$dest.tmp" "$dest"
	fetched=$((fetched + 1))
}

# URL-encode spaces only; upstream paths use no other unsafe characters.
urlenc() { printf '%s' "$1" | sed 's/ /%20/g'; }

for entry in "${KENNEY_PACKS[@]}"; do
	IFS='|' read -r pack repo commit dir <<<"$entry"
	mkdir -p "$MODELS/$pack"
	n=$(pack_count "$pack")
	echo "kenney/$pack ($n models)"
	while read -r name sha; do
		[[ -z "$name" ]] && continue
		url="https://raw.githubusercontent.com/KenneyNL/$repo/$commit/$(urlenc "$dir/$name")"
		fetch "$url" "$MODELS/$pack/$name" "$sha" "kenney/$pack/$name"
	done < <(pack_files "$pack")
done

mkdir -p "$CHARS"
echo "kaykit/characters (1 model)"
fetch "$KAYKIT_BASE/$KAYKIT_DIR/Knight.glb" "$CHARS/knight.glb" \
	60428e3abc09ba83e595d256e3af8c5c976b46cdae599f0802fc82b4a3445168 "kaykit/knight.glb"
fetch "$KAYKIT_BASE/$KAYKIT_DIR/knight_texture.png" "$CHARS/knight_texture.png" \
	5d250ccc5da020e6126bfa3839f83bd9a465a951ed223e4d13c08b1925e154d4 "kaykit/knight_texture.png"

# ---- 3D model packs (manifest-driven) -------------------------------------
if [[ -f "$MANIFEST" ]]; then
	while IFS='|' read -r slug url sha models core usable; do
		[[ -z "$slug" ]] && continue
		[[ "$usable" == "true" ]] || continue
		want_pack "$slug" "$core" || continue
		dest="$MODELS/$slug"
		# The marker is written only after a COMPLETE extraction (models plus
		# any texture atlas). Counting models alone declared a pack cached when
		# its colormap.png had never been extracted at all — 48 of 52 packs
		# rendered untextured while the script cheerfully reported them cached.
		if [[ -f "$dest/.extracted" ]] && [[ $(find "$dest" -iname '*.glb' -o -iname '*.gltf' | wc -l | tr -d ' ') -ge "$models" ]]; then
			cached=$((cached + 1))
			continue
		fi
		# A mirror serves <base>/<slug>.zip; upstream keeps its own layout.
		src="$url"
		[[ -n "$MIRROR" ]] && src="$MIRROR/$slug.zip"
		echo "kenney/$slug ($models models)"
		zip="$MODELS/.$slug.zip"
		mkdir -p "$MODELS"
		fetch "$src" "$zip" "$sha" "models/$slug"
		mkdir -p "$dest"
		unzip -qo "$zip" -d "$zip.d" 2>/dev/null
		chmod -R u+w "$zip.d" 2>/dev/null
		# Prefer GLB and skip the OBJ/FBX/DAE copies — they are most of the
		# archive and we cannot load them. NOTE GLB is NOT self-contained here:
		# Kenney materials reference an external Textures/colormap.png shared
		# by the whole pack, so the PNGs come too (they are tiny — 212 atlases
		# total about 42 KB). Without them every model renders white.
		find "$zip.d" -iname '*.png' -exec sh -c 'mv -f "$1" "$2/$(basename "$1")"' _ {} "$dest" \;
		if [[ $(find "$zip.d" -iname '*.glb' | wc -l | tr -d ' ') -gt 0 ]]; then
			find "$zip.d" -iname '*.glb' -exec sh -c 'mv -f "$1" "$2/$(basename "$1")"' _ {} "$dest" \;
		else
			find "$zip.d" \( -iname '*.gltf' -o -iname '*.bin' \) -exec sh -c 'mv -f "$1" "$2/$(basename "$1")"' _ {} "$dest" \;
		fi
		rm -rf "$zip.d" "$zip"
		: >"$dest/.extracted"
		# Be a good guest: pace requests so a full run is a trickle, not a flood.
		sleep 1
	done < <(manifest_rows)
fi

for entry in "${KENNEY_AUDIO[@]}"; do
	IFS='|' read -r pack hash sha count <<<"$entry"
	dest="$AUDIO/$pack"
	zip="$AUDIO/.$pack.zip"
	if [[ -d "$dest" ]] && [[ $(find "$dest" -name '*.ogg' | wc -l | tr -d ' ') -ge "$count" ]]; then
		cached=$((cached + 1))
		continue
	fi
	echo "kenney/$pack ($count sounds)"
	mkdir -p "$AUDIO"
	fetch "https://kenney.nl/media/pages/assets/$pack/$hash/kenney_$pack.zip" "$zip" "$sha" "audio/$pack"
	mkdir -p "$dest"
	# Flatten: the packs nest under Audio/ or Sounds/, and we only want the
	# sound files — never the bundled .url/.txt/preview cruft.
	unzip -qo "$zip" -d "$zip.d"
	# Some packs ship directories without the owner write bit (sci-fi-sounds
	# is mode r-xr-xr-x), which blocks both the move out and the cleanup.
	chmod -R u+w "$zip.d"
	find "$zip.d" -name '*.ogg' -exec mv -f {} "$dest/" \;
	rm -rf "$zip.d" "$zip"
done

if [[ $TRANSCODE == 1 ]]; then
	if command -v ffmpeg >/dev/null 2>&1; then
		echo "transcoding ogg -> wav (ffmpeg)"
		find "$AUDIO" -name '*.ogg' | while read -r f; do
			w="${f%.ogg}.wav"
			[[ -f "$w" ]] || ffmpeg -loglevel error -y -i "$f" "$w"
		done
	else
		echo "WARNING: --transcode needs ffmpeg on PATH; skipping" >&2
	fi
fi

echo
echo "done — $fetched fetched, $cached already cached"
echo
echo "These assets are CC0 (public domain). Attribution isn't required, but is"
echo "deserved:"
echo "  Kenney            https://kenney.nl/assets"
echo "  KayKit / Kay Lousberg  https://kaylousberg.itch.io/"
echo
echo "Thank you both for giving so many high-quality assets away for free."
