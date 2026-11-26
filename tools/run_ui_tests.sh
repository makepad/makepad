#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "makepad UI suites are currently validated on macOS only."
  exit 1
fi

packages=(
  "makepad-example-text-input:examples/text_input"
  "makepad-example-counter:examples/counter"
  "makepad-example-todo:examples/todo"
  "makepad-example-floating-panel:examples/floating_panel"
  "makepad-example-splash:examples/splash"
)

passed=()
failed=()

target_dir="${CARGO_TARGET_DIR:-$ROOT_DIR/target}"
if [[ "$target_dir" != /* ]]; then
  # Cargo starts each test in its package directory. Pass the same absolute target to both.
  target_dir="$ROOT_DIR/$target_dir"
  export CARGO_TARGET_DIR="$target_dir"
fi

for entry in "${packages[@]}"; do
  package="${entry%%:*}"
  rel_dir="${entry#*:}"
  artifact_root="${MAKEPAD_TEST_ARTIFACTS:-$target_dir/makepad_test}"
  if [[ "$artifact_root" != /* ]]; then
    artifact_root="$ROOT_DIR/$rel_dir/$artifact_root"
  fi
  artifact_dir="$artifact_root/$package"

  echo
  echo "==> $package"
  echo "    artifacts: $artifact_dir"

  if cargo test -p "$package" --test ui -- --test-threads=1; then
    passed+=("$package")
  else
    failed+=("$package ($artifact_dir)")
  fi
done

echo
echo "UI suite summary"
echo "  passed: ${#passed[@]}"
if ((${#passed[@]} > 0)); then
  printf '    - %s\n' "${passed[@]}"
fi

echo "  failed: ${#failed[@]}"
if ((${#failed[@]} > 0)); then
  printf '    - %s\n' "${failed[@]}"
  exit 1
fi
