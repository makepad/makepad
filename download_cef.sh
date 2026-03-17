#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
INDEX_URL="https://cef-builds.spotifycdn.com/index.json"
BASE_URL="https://cef-builds.spotifycdn.com"
OUT_DIR="${ROOT_DIR}/local/cef-prebuilt"
CHANNEL="stable"
FILE_TYPE="standard"
VERSION="latest"
PLATFORM="host"
DOWNLOAD_ONLY=0
FORCE=0

usage() {
    cat <<'EOF'
Usage: ./download_cef.sh [options]

Downloads the official prebuilt CEF binary distribution from the public
CEF builds index and extracts it into local/cef-prebuilt by default.

Options:
  --platform <name>   Target platform, a comma-separated list, or "all".
                      Defaults to the current host platform.
                      Supported desktop platforms:
                      linux64, linuxarm64, macosarm64, macosx64,
                      windows64, windowsarm64
  --version <value>   Exact CEF version or "latest" (default)
  --channel <value>   stable or beta (default: stable)
  --type <value>      standard, minimal, or client (default: standard)
  --out <dir>         Output directory (default: local/cef-prebuilt)
  --download-only     Download archive but do not extract it
  --force             Re-download archive and re-extract it
  -h, --help          Show this help

Examples:
  ./download_cef.sh
  ./download_cef.sh --platform macosarm64
  ./download_cef.sh --platform all --type standard
  ./download_cef.sh --platform windows64,linux64 --channel beta
EOF
}

host_platform() {
    local os machine
    os="$(uname -s)"
    machine="$(uname -m)"

    case "${os}:${machine}" in
        Darwin:arm64|Darwin:aarch64)
            printf '%s\n' "macosarm64"
            ;;
        Darwin:x86_64)
            printf '%s\n' "macosx64"
            ;;
        Linux:x86_64)
            printf '%s\n' "linux64"
            ;;
        Linux:arm64|Linux:aarch64)
            printf '%s\n' "linuxarm64"
            ;;
        MINGW*:x86_64|MSYS*:x86_64|CYGWIN*:x86_64)
            printf '%s\n' "windows64"
            ;;
        MINGW*:arm64|MSYS*:arm64|CYGWIN*:arm64)
            printf '%s\n' "windowsarm64"
            ;;
        *)
            printf 'Unsupported host platform: %s %s\n' "${os}" "${machine}" >&2
            exit 1
            ;;
    esac
}

sha1_file() {
    local path="$1"
    python3 - "$path" <<'PY'
import hashlib
import pathlib
import sys

path = pathlib.Path(sys.argv[1])
sha1 = hashlib.sha1()
with path.open('rb') as handle:
    for chunk in iter(lambda: handle.read(1024 * 1024), b''):
        sha1.update(chunk)
print(sha1.hexdigest())
PY
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --platform)
            PLATFORM="$2"
            shift 2
            ;;
        --version)
            VERSION="$2"
            shift 2
            ;;
        --channel)
            CHANNEL="$2"
            shift 2
            ;;
        --type)
            FILE_TYPE="$2"
            shift 2
            ;;
        --out)
            OUT_DIR="$2"
            shift 2
            ;;
        --download-only)
            DOWNLOAD_ONLY=1
            shift
            ;;
        --force)
            FORCE=1
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            printf 'Unknown option: %s\n' "$1" >&2
            usage >&2
            exit 1
            ;;
    esac
done

case "${CHANNEL}" in
    stable|beta)
        ;;
    *)
        printf 'Unsupported channel: %s\n' "${CHANNEL}" >&2
        exit 1
        ;;
esac

case "${FILE_TYPE}" in
    standard|minimal|client)
        ;;
    *)
        printf 'Unsupported distribution type: %s\n' "${FILE_TYPE}" >&2
        exit 1
        ;;
esac

if [[ "${PLATFORM}" == "host" ]]; then
    PLATFORM="$(host_platform)"
fi

mkdir -p "${OUT_DIR}"

index_json="$(mktemp "${TMPDIR:-/tmp}/cef-index.XXXXXX.json")"
trap 'rm -f "${index_json}"' EXIT

echo "Fetching CEF index..."
curl -L --fail --retry 3 --retry-delay 2 -o "${index_json}" "${INDEX_URL}"

resolved=()
while IFS= read -r line; do
    [[ -n "${line}" ]] && resolved+=("${line}")
done < <(
    python3 - "${index_json}" "${PLATFORM}" "${VERSION}" "${CHANNEL}" "${FILE_TYPE}" <<'PY'
import json
import sys

index_path, platform_arg, version_arg, channel_arg, type_arg = sys.argv[1:]
supported = [
    "linux64",
    "linuxarm64",
    "macosarm64",
    "macosx64",
    "windows64",
    "windowsarm64",
]

with open(index_path, "r", encoding="utf-8") as handle:
    data = json.load(handle)

if platform_arg == "all":
    platforms = supported
else:
    platforms = [p.strip() for p in platform_arg.split(",") if p.strip()]

for platform in platforms:
    if platform not in supported:
        raise SystemExit(f"Unsupported platform: {platform}")
    if platform not in data:
        raise SystemExit(f"Platform not present in CEF index: {platform}")

    selected_version = None
    selected_file = None
    for version_entry in data[platform]["versions"]:
        if version_entry.get("channel", "stable") != channel_arg:
            continue
        if version_arg != "latest" and version_entry["cef_version"] != version_arg:
            continue
        for file_entry in version_entry["files"]:
            if file_entry["type"] == type_arg:
                selected_version = version_entry
                selected_file = file_entry
                break
        if selected_file is not None:
            break

    if selected_file is None:
        raise SystemExit(
            f"No {type_arg} archive found for platform={platform} "
            f"channel={channel_arg} version={version_arg}"
        )

    print(
        "\t".join(
            [
                platform,
                selected_version["cef_version"],
                selected_file["name"],
                selected_file["sha1"],
                str(selected_file["size"]),
                selected_version.get("channel", "stable"),
                selected_file["type"],
            ]
        )
    )
PY
)

if [[ ${#resolved[@]} -eq 0 ]]; then
    echo "No matching CEF distributions found." >&2
    exit 1
fi

for entry in "${resolved[@]}"; do
    IFS=$'\t' read -r platform cef_version archive_name archive_sha1 archive_size archive_channel archive_type <<<"${entry}"

    archive_path="${OUT_DIR}/${archive_name}"
    extract_dir="${OUT_DIR}/${archive_name%.tar.bz2}"
    current_link="${OUT_DIR}/current-${platform}"
    archive_url="${BASE_URL}/${archive_name}"

    echo
    echo "Platform : ${platform}"
    echo "Version  : ${cef_version}"
    echo "Channel  : ${archive_channel}"
    echo "Type     : ${archive_type}"
    echo "Archive  : ${archive_name}"
    echo "Size     : ${archive_size} bytes"

    if [[ ${FORCE} -eq 1 ]]; then
        rm -f "${archive_path}"
        rm -rf "${extract_dir}"
    fi

    if [[ -f "${archive_path}" ]]; then
        echo "Using existing archive: ${archive_path}"
    else
        tmp_archive="${archive_path}.part"
        rm -f "${tmp_archive}"
        echo "Downloading ${archive_url}"
        curl -L --fail --retry 3 --retry-delay 2 -o "${tmp_archive}" "${archive_url}"
        mv "${tmp_archive}" "${archive_path}"
    fi

    echo "Verifying SHA-1..."
    actual_sha1="$(sha1_file "${archive_path}")"
    if [[ "${actual_sha1}" != "${archive_sha1}" ]]; then
        printf 'SHA-1 mismatch for %s\nexpected: %s\nactual:   %s\n' \
            "${archive_name}" "${archive_sha1}" "${actual_sha1}" >&2
        exit 1
    fi

    if [[ ${DOWNLOAD_ONLY} -eq 1 ]]; then
        echo "Downloaded: ${archive_path}"
        continue
    fi

    if [[ -d "${extract_dir}" ]]; then
        echo "Using existing extract dir: ${extract_dir}"
    else
        echo "Extracting ${archive_name}..."
        tar -xjf "${archive_path}" -C "${OUT_DIR}"
    fi

    ln -sfn "${extract_dir}" "${current_link}"
    echo "Ready: ${extract_dir}"
    echo "Link : ${current_link}"
done
