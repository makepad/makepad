#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
ROOT="$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)"
MAP_DIR="${MAP_DIR:-$ROOT/local/maps}"

# Pinned, immutable upstream snapshots. Override these variables deliberately
# when refreshing the repository's local data.
GEOFABRIK_DATE="${GEOFABRIK_DATE:-260726}"
VERSATILES_DATE="${VERSATILES_DATE:-20260608}"

RAW_NAME="europe-${GEOFABRIK_DATE}.osm.pbf"
RAW_URL="https://download.geofabrik.de/europe-${GEOFABRIK_DATE}.osm.pbf"
RAW_PATH="$MAP_DIR/$RAW_NAME"
RAW_BYTES="${RAW_BYTES:-34701836396}"
RAW_POLY_PATH="$MAP_DIR/europe.poly"

TILES_SOURCE_NAME="osm.${VERSATILES_DATE}.versatiles"
TILES_SOURCE_URL="https://download.versatiles.org/$TILES_SOURCE_NAME"
TILES_SOURCE_PATH="$MAP_DIR/$TILES_SOURCE_NAME"
TILES_SOURCE_BYTES="${TILES_SOURCE_BYTES:-66534652244}"

MBTILES_PATH="${MBTILES_PATH:-$MAP_DIR/europe-shortbread.mbtiles}"
DETAIL_MBTILES_PATH="${DETAIL_MBTILES_PATH:-$MAP_DIR/europe-osm-detail.mbtiles}"
PBF_AUDIT_PATH="${PBF_AUDIT_PATH:-$MAP_DIR/europe-pbf-audit.txt}"
DETAIL_STORE="${DETAIL_STORE:-$MAP_DIR/native-detail-europe.store}"
DETAIL_ZOOM="${DETAIL_ZOOM:-14}"
DETAIL_SORT_MEMORY_MIB="${DETAIL_SORT_MEMORY_MIB:-256}"

# Bounding rectangle of Geofabrik's Europe polygon. The rectangle intentionally
# includes a little surrounding data, so it is guaranteed to cover that polygon.
EUROPE_BBOX="${EUROPE_BBOX:--32.683233,29.635548,46.753480,81.472990}"

usage() {
    cat <<EOF
Usage: ./tools/download_map.sh [all|raw|tiles|plan|convert|audit|detail|verify]

  all      Download both sources and create the curated Europe base (default)
  raw      Download the pinned 34.7 GB Geofabrik Europe .osm.pbf
  tiles    Download the pinned 66.5 GB full-planet Shortbread .versatiles
  plan     Validate the tiled source indexes and report Europe output size
  convert  Create the Europe Shortbread base MBTiles
  audit    Stream the raw PBF and write a source completeness manifest
  detail   Create all-tag tiled detail MBTiles from the raw PBF in native Rust
  verify   Check downloaded sources and any generated MBTiles files

Environment overrides:
  MAP_DIR, MBTILES_PATH, GEOFABRIK_DATE, VERSATILES_DATE, EUROPE_BBOX
  RAW_BYTES, TILES_SOURCE_BYTES
  DETAIL_MBTILES_PATH, PBF_AUDIT_PATH
  DETAIL_STORE, DETAIL_ZOOM, DETAIL_SORT_MEMORY_MIB

Outputs:
  $RAW_PATH
  $RAW_POLY_PATH
  $TILES_SOURCE_PATH
  $MBTILES_PATH
  $DETAIL_MBTILES_PATH
  $PBF_AUDIT_PATH

The Shortbread file is the styled/generalised z0-14 base. It does not contain
every source tag. The detail action independently converts the raw PBF and
retains every tagged spatial element and all tags. It is intentionally not part
of 'all': Europe detail generation needs substantial SSD space and runtime.
EOF
}

sha256_file() {
    if command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$1" | awk '{print $1}'
    elif command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{print $1}'
    else
        echo "Need shasum or sha256sum to verify downloads" >&2
        return 1
    fi
}

md5_file() {
    if command -v md5 >/dev/null 2>&1; then
        md5 -q "$1"
    elif command -v md5sum >/dev/null 2>&1; then
        md5sum "$1" | awk '{print $1}'
    else
        echo "Need md5 or md5sum to verify downloads" >&2
        return 1
    fi
}

download_sidecar() {
    local url="$1"
    local output="$2"
    curl -L \
        --fail \
        --show-error \
        --retry 10 \
        --retry-delay 3 \
        --retry-all-errors \
        --output "$output" \
        "$url"
}

download_payload() {
    local url="$1"
    local output="$2"
    local expected_bytes="$3"
    local partial="${output}.part"
    local attempt=1
    local max_attempts=20
    local actual_bytes

    echo "Downloading $(basename "$output")"
    echo "  source: $url"
    echo "  partial: $partial"
    while (( attempt <= max_attempts )); do
        if [[ -f "$partial" ]]; then
            actual_bytes="$(wc -c < "$partial" | tr -d '[:space:]')"
            if [[ "$actual_bytes" == "$expected_bytes" ]]; then
                return
            fi
            if (( actual_bytes > expected_bytes )); then
                echo "Partial file is $actual_bytes bytes; expected at most $expected_bytes" >&2
                return 1
            fi
        fi

        # Run one curl process per attempt. Curl's built-in retry can retain the
        # resume offset from the start of the process and truncate progress made
        # before a later transport failure.
        if curl -L \
            --http1.1 \
            --fail \
            --show-error \
            --connect-timeout 30 \
            --speed-limit 1024 \
            --speed-time 60 \
            --continue-at - \
            --output "$partial" \
            "$url"
        then
            actual_bytes="$(wc -c < "$partial" | tr -d '[:space:]')"
            if [[ "$actual_bytes" == "$expected_bytes" ]]; then
                return
            fi
            echo "Downloaded size is $actual_bytes bytes; expected $expected_bytes" >&2
        else
            echo "Download attempt $attempt/$max_attempts failed" >&2
        fi

        attempt=$((attempt + 1))
        if (( attempt <= max_attempts )); then
            echo "Resuming in 5 seconds" >&2
            sleep 5
        fi
    done
    echo "Download failed after $max_attempts attempts" >&2
    return 1
}

download_raw() {
    local checksum_path="${RAW_PATH}.md5"
    local expected
    local actual

    download_sidecar "https://download.geofabrik.de/europe.poly" "$RAW_POLY_PATH"
    download_sidecar "${RAW_URL}.md5" "$checksum_path"
    expected="$(awk 'NR == 1 {print $1}' "$checksum_path")"
    if [[ -z "$expected" ]]; then
        echo "Empty checksum in $checksum_path" >&2
        return 1
    fi

    if [[ -f "$RAW_PATH" ]]; then
        actual="$(md5_file "$RAW_PATH")"
        if [[ "$actual" == "$expected" ]]; then
            echo "Verified existing $RAW_PATH"
            return
        fi
        echo "Existing $RAW_PATH has the wrong MD5; refusing to overwrite it" >&2
        return 1
    fi

    download_payload "$RAW_URL" "$RAW_PATH" "$RAW_BYTES"
    actual="$(md5_file "${RAW_PATH}.part")"
    if [[ "$actual" != "$expected" ]]; then
        echo "MD5 mismatch for ${RAW_PATH}.part" >&2
        echo "expected: $expected" >&2
        echo "actual:   $actual" >&2
        return 1
    fi
    mv "${RAW_PATH}.part" "$RAW_PATH"
    echo "Verified $RAW_PATH"
}

download_tiles_source() {
    local checksum_path="${TILES_SOURCE_PATH}.sha256"
    local expected
    local actual

    download_sidecar "${TILES_SOURCE_URL}.sha256" "$checksum_path"
    expected="$(awk 'NR == 1 {print $1}' "$checksum_path")"
    if [[ -z "$expected" ]]; then
        echo "Empty checksum in $checksum_path" >&2
        return 1
    fi

    if [[ -f "$TILES_SOURCE_PATH" ]]; then
        actual="$(sha256_file "$TILES_SOURCE_PATH")"
        if [[ "$actual" == "$expected" ]]; then
            echo "Verified existing $TILES_SOURCE_PATH"
            return
        fi
        echo "Existing $TILES_SOURCE_PATH has the wrong SHA-256; refusing to overwrite it" >&2
        return 1
    fi

    download_payload "$TILES_SOURCE_URL" "$TILES_SOURCE_PATH" "$TILES_SOURCE_BYTES"
    actual="$(sha256_file "${TILES_SOURCE_PATH}.part")"
    if [[ "$actual" != "$expected" ]]; then
        echo "SHA-256 mismatch for ${TILES_SOURCE_PATH}.part" >&2
        echo "expected: $expected" >&2
        echo "actual:   $actual" >&2
        return 1
    fi
    mv "${TILES_SOURCE_PATH}.part" "$TILES_SOURCE_PATH"
    echo "Verified $TILES_SOURCE_PATH"
}

convert_tiles() {
    local converter="$ROOT/target/release/makepad-map-tiles"
    local partial="${MBTILES_PATH%.mbtiles}.partial.mbtiles"

    if [[ ! -f "$TILES_SOURCE_PATH" ]]; then
        echo "Missing $TILES_SOURCE_PATH; run ./tools/download_map.sh tiles first" >&2
        return 1
    fi
    if [[ -f "$MBTILES_PATH" ]]; then
        validate_mbtiles "$MBTILES_PATH"
        echo "Keeping existing $MBTILES_PATH"
        return
    fi
    if [[ -e "$partial" ]]; then
        echo "Refusing to overwrite incomplete output $partial" >&2
        echo "Move or remove it explicitly before retrying." >&2
        return 1
    fi

    echo "Building the map converter in release mode"
    cargo build --release --package makepad-map-tiles \
        --manifest-path "$ROOT/Cargo.toml"

    echo "Creating Europe Shortbread MBTiles"
    "$converter" \
        "$TILES_SOURCE_PATH" \
        "$partial" \
        --bbox "$EUROPE_BBOX" \
        --max-zoom 14

    if command -v sqlite3 >/dev/null 2>&1; then
        local check
        check="$(sqlite3 -readonly "$partial" "PRAGMA quick_check;")"
        if [[ "$check" != "ok" ]]; then
            echo "SQLite validation failed for $partial: $check" >&2
            return 1
        fi
    fi
    "$converter" probe-mbtiles "$partial" 14/8414/5384

    mv "$partial" "$MBTILES_PATH"
    echo "Ready: $MBTILES_PATH"
}

plan_tiles() {
    local converter="$ROOT/target/release/makepad-map-tiles"

    if [[ ! -f "$TILES_SOURCE_PATH" ]]; then
        echo "Missing $TILES_SOURCE_PATH; run ./tools/download_map.sh tiles first" >&2
        return 1
    fi

    cargo build --release --package makepad-map-tiles \
        --manifest-path "$ROOT/Cargo.toml"
    "$converter" \
        "$TILES_SOURCE_PATH" \
        "$MBTILES_PATH" \
        --bbox "$EUROPE_BBOX" \
        --max-zoom 14 \
        --plan-only
}

validate_mbtiles() {
    local path="$1"
    local check
    local tables

    if [[ ! -f "$path" ]]; then
        echo "Missing $path" >&2
        return 1
    fi
    if ! command -v sqlite3 >/dev/null 2>&1; then
        echo "Need sqlite3 to validate $path" >&2
        return 1
    fi

    check="$(sqlite3 -readonly "$path" "PRAGMA quick_check;")"
    if [[ "$check" != "ok" ]]; then
        echo "SQLite validation failed for $path: $check" >&2
        return 1
    fi
    tables="$(sqlite3 -readonly "$path" \
        "SELECT group_concat(name, ',') FROM sqlite_master WHERE type='table' AND name IN ('metadata','tiles') ORDER BY name;")"
    if [[ "$tables" != "metadata,tiles" && "$tables" != "tiles,metadata" ]]; then
        echo "$path does not expose both MBTiles tables (found: $tables)" >&2
        return 1
    fi
    echo "Validated $path"
}

audit_raw() {
    local converter="$ROOT/target/release/makepad-map-tiles"
    local partial="${PBF_AUDIT_PATH}.partial"

    if [[ ! -f "$RAW_PATH" ]]; then
        echo "Missing $RAW_PATH; run ./tools/download_map.sh raw first" >&2
        return 1
    fi
    if [[ -f "$PBF_AUDIT_PATH" ]]; then
        if grep -qx "source_bytes=$RAW_BYTES" "$PBF_AUDIT_PATH"; then
            echo "Keeping existing source audit $PBF_AUDIT_PATH"
            return
        fi
        echo "Refusing to overwrite stale audit $PBF_AUDIT_PATH" >&2
        return 1
    fi
    if [[ -e "$partial" ]]; then
        echo "Refusing to overwrite incomplete audit $partial" >&2
        return 1
    fi

    cargo build --release --package makepad-map-tiles \
        --manifest-path "$ROOT/Cargo.toml"
    "$converter" audit-pbf "$RAW_PATH" | tee "$partial"
    mv "$partial" "$PBF_AUDIT_PATH"
    echo "Ready: $PBF_AUDIT_PATH"
}

convert_detail() {
    local converter="$ROOT/target/release/makepad-map-tiles"
    local partial="${DETAIL_MBTILES_PATH%.mbtiles}.partial.mbtiles"
    local probe
    local probe_z
    local probe_x
    local probe_tms_y
    local probe_y

    if [[ ! -f "$RAW_PATH" ]]; then
        echo "Missing $RAW_PATH; run ./tools/download_map.sh raw first" >&2
        return 1
    fi
    if [[ -f "$DETAIL_MBTILES_PATH" ]]; then
        validate_mbtiles "$DETAIL_MBTILES_PATH"
        echo "Keeping existing $DETAIL_MBTILES_PATH"
        return
    fi
    if [[ -e "$partial" ]]; then
        echo "Refusing to overwrite incomplete output $partial" >&2
        echo "Move or remove it explicitly before retrying." >&2
        return 1
    fi
    if [[ -e "$DETAIL_STORE" ]]; then
        if [[ -f "$DETAIL_STORE/spool.complete.json" ]]; then
            echo "Reusing completed native scratch store $DETAIL_STORE"
        else
            echo "Native scratch store is incomplete: $DETAIL_STORE" >&2
            echo "Move it aside or remove it explicitly before restarting." >&2
            return 1
        fi
    fi
    mkdir -p "$(dirname "$DETAIL_MBTILES_PATH")" "$(dirname "$DETAIL_STORE")"

    echo "Building the native map converter in release mode"
    cargo build --release --package makepad-map-tiles \
        --manifest-path "$ROOT/Cargo.toml"

    cat <<EOF
Creating the all-tag Europe street-detail archive
  input:   $RAW_PATH
  output:  $partial
  store:   $DETAIL_STORE
  zoom:    $DETAIL_ZOOM

This is a large offline conversion. Every tagged spatial object and every tag
is retained. Closed ways remain both lines and polygon candidates, relation
geometries are assembled, and coordinates are clipped to buffered integer MVT
tiles. The raw PBF remains the canonical copy for the complete OSM graph.
EOF

    "$converter" pbf-detail \
        "$RAW_PATH" \
        "$partial" \
        --store "$DETAIL_STORE" \
        --zoom "$DETAIL_ZOOM" \
        --sort-memory-mib "$DETAIL_SORT_MEMORY_MIB"

    validate_mbtiles "$partial"
    if ! sqlite3 -readonly "$partial" \
        "SELECT value FROM metadata WHERE name='makepad_source_kind';" \
        | grep -qx "osm-all-tags-native-detail-v1"
    then
        echo "Detail MBTiles lacks the native all-tag source marker" >&2
        return 1
    fi
    probe="$(sqlite3 -readonly "$partial" \
        "SELECT zoom_level, tile_column, tile_row FROM tiles ORDER BY length(tile_data) DESC LIMIT 1;")"
    IFS='|' read -r probe_z probe_x probe_tms_y <<< "$probe"
    if [[ -z "$probe_z" || -z "$probe_x" || -z "$probe_tms_y" ]]; then
        echo "Detail MBTiles contains no probeable tile" >&2
        return 1
    fi
    probe_y=$(((1 << probe_z) - 1 - probe_tms_y))
    "$converter" probe-mbtiles "$partial" "$probe_z/$probe_x/$probe_y"

    mv "$partial" "$DETAIL_MBTILES_PATH"
    echo "Ready: $DETAIL_MBTILES_PATH"
    echo "Native scratch and audit retained at $DETAIL_STORE"
}

verify_all() {
    local failed=0

    if [[ -f "$RAW_PATH" && -f "${RAW_PATH}.md5" ]]; then
        local raw_expected
        raw_expected="$(awk 'NR == 1 {print $1}' "${RAW_PATH}.md5")"
        if [[ "$(md5_file "$RAW_PATH")" == "$raw_expected" ]]; then
            echo "Verified $RAW_PATH"
        else
            echo "MD5 mismatch: $RAW_PATH" >&2
            failed=1
        fi
    else
        echo "Missing raw source or checksum: $RAW_PATH" >&2
        failed=1
    fi

    if [[ -f "$TILES_SOURCE_PATH" && -f "${TILES_SOURCE_PATH}.sha256" ]]; then
        local tiles_expected
        tiles_expected="$(awk 'NR == 1 {print $1}' "${TILES_SOURCE_PATH}.sha256")"
        if [[ "$(sha256_file "$TILES_SOURCE_PATH")" == "$tiles_expected" ]]; then
            echo "Verified $TILES_SOURCE_PATH"
        else
            echo "SHA-256 mismatch: $TILES_SOURCE_PATH" >&2
            failed=1
        fi
    else
        echo "Missing tiled source or checksum: $TILES_SOURCE_PATH" >&2
        failed=1
    fi

    if [[ -f "$MBTILES_PATH" ]]; then
        validate_mbtiles "$MBTILES_PATH" || failed=1
    fi
    if [[ -f "$DETAIL_MBTILES_PATH" ]]; then
        validate_mbtiles "$DETAIL_MBTILES_PATH" || failed=1
    fi
    return "$failed"
}

main() {
    local action="${1:-all}"
    mkdir -p "$MAP_DIR"

    case "$action" in
        all)
            download_raw
            download_tiles_source
            convert_tiles
            ;;
        raw)
            download_raw
            ;;
        tiles)
            download_tiles_source
            ;;
        plan)
            plan_tiles
            ;;
        convert)
            convert_tiles
            ;;
        audit)
            audit_raw
            ;;
        detail)
            convert_detail
            ;;
        verify)
            verify_all
            ;;
        -h|--help|help)
            usage
            ;;
        *)
            echo "Unknown action: $action" >&2
            usage >&2
            return 2
            ;;
    esac
}

main "$@"
