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

for entry in "${packages[@]}"; do
  package="${entry%%:*}"
  rel_dir="${entry#*:}"
  artifact_dir="$ROOT_DIR/$rel_dir/target/makepad_test/$package"

  echo
  echo "==> $package"
  echo "    artifacts: $artifact_dir"

  if cargo test -p "$package" --test ui -- --test-threads=1; then
    passed+=("$package")
  else
    failed+=("$package")
  fi
done

echo
echo "UI suite summary"
echo "  passed: ${#passed[@]}"
for package in "${passed[@]}"; do
  echo "    - $package"
done

echo "  failed: ${#failed[@]}"
for package in "${failed[@]}"; do
  case "$package" in
    makepad-example-text-input) rel_dir="examples/text_input" ;;
    makepad-example-counter) rel_dir="examples/counter" ;;
    makepad-example-todo) rel_dir="examples/todo" ;;
    makepad-example-floating-panel) rel_dir="examples/floating_panel" ;;
    makepad-example-splash) rel_dir="examples/splash" ;;
    *) rel_dir="." ;;
  esac
  echo "    - $package ($ROOT_DIR/$rel_dir/target/makepad_test/$package)"
done

if ((${#failed[@]} > 0)); then
  exit 1
fi
