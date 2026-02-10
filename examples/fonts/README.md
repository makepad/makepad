# makepad-example-fonts

Test app for Makepad's system font support.

## Running

```bash
# With bundled fonts (default)
cargo run -p makepad-fonts --release

# With system fonts
cargo run -p makepad-fonts --no-default-features --features system-fonts --release
```

## What it Shows

- Multilingual text: Chinese, Japanese, Russian, English
- System font rendering on macOS/Windows/Linux

## Related

- `docs/plans/2026-01-22-system-font-support.md` - Implementation docs
- `platform/src/os/*/system_fonts.rs` - Platform providers
