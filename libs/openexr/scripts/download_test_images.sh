#!/usr/bin/env bash
set -euo pipefail

dest_root="${1:-${MAKEPAD_OPENEXR_TESTDATA_DIR:-/tmp/makepad-openexr-test-images}}"
base_url="https://raw.githubusercontent.com/AcademySoftwareFoundation/openexr-images/main"

files=(
  "ScanLines/Blobbies.exr"
  "Beachball/singlepart.0001.exr"
  "Beachball/multipart.0001.exr"
)

mkdir -p "${dest_root}"

for rel_path in "${files[@]}"; do
  out_path="${dest_root}/${rel_path}"
  mkdir -p "$(dirname "${out_path}")"
  echo "downloading ${rel_path}"
  curl --location --fail --silent --show-error \
    --output "${out_path}" \
    "${base_url}/${rel_path}"
done

echo "OpenEXR test images downloaded to ${dest_root}"
