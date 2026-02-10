# System Font Support

**Status:** Complete
**Date:** 2026-01-22

## Overview

This document describes the changes made to enable Makepad applications to use operating system fonts instead of bundled fonts. This reduces distribution size and allows apps to use native system fonts for non-Latin text rendering.

## Architecture

### Font Loading Modes

| Mode | Feature Flag | Description |
|------|--------------|-------------|
| Bundled (default) | `bundled-fonts` | Embeds Chinese and emoji fonts from separate crates |
| System | `system-fonts` | Uses OS font providers for fallback |

### Platform-Specific Font Providers

Platform-specific implementations of `SystemFontProvider` trait:

| Platform | Backend | File |
|----------|---------|------|
| macOS/iOS | Core Text | `platform/src/os/apple/system_fonts.rs` |
| Windows | DirectWrite | `platform/src/os/windows/system_fonts.rs` |
| Linux | Fontconfig | `platform/src/os/linux/system_fonts.rs` |

```rust
pub trait SystemFontProvider: Send + Sync {
    fn query_font(&self, family: &str) -> Result<SystemFontData, SystemFontError>;
    fn list_families(&self) -> Vec<String>;
}
```

## Changes Made

### 1. widgets/Cargo.toml

Made Chinese and emoji font crates optional, gated behind `bundled-fonts` feature:

```toml
[dependencies]
makepad-draw = { path = "../draw", version = "1.0.0", default-features = false }

makepad-fonts-emoji = {path = "./fonts/emoji", version = "1.0.0", optional = true}
makepad-fonts-chinese-regular = {path = "./fonts/chinese_regular", version = "1.0.1", optional = true}
makepad-fonts-chinese-regular-2 = {path = "./fonts/chinese_regular_2", version = "1.0.1", optional = true}
makepad-fonts-chinese-bold = {path = "./fonts/chinese_bold", version = "1.0.1", optional = true}
makepad-fonts-chinese-bold-2 = {path = "./fonts/chinese_bold_2", version = "1.0.1", optional = true}

[features]
default = ["bundled-fonts"]
bundled-fonts = [
    "makepad-draw/bundled-fonts",
    "dep:makepad-fonts-emoji",
    "dep:makepad-fonts-chinese-regular",
    "dep:makepad-fonts-chinese-regular-2",
    "dep:makepad-fonts-chinese-bold",
    "dep:makepad-fonts-chinese-bold-2",
]
system-fonts = ["makepad-draw/system-fonts"]
```

### 2. widgets/src/lib.rs

Conditional font crate initialization:

```rust
#[cfg(feature = "bundled-fonts")]
{
    makepad_fonts_emoji::live_design(cx);
    makepad_fonts_chinese_regular::live_design(cx);
    makepad_fonts_chinese_regular_2::live_design(cx);
    makepad_fonts_chinese_bold::live_design(cx);
    makepad_fonts_chinese_bold_2::live_design(cx);
}
```

### 3. code_editor/Cargo.toml

Added feature flag propagation:

```toml
[dependencies]
makepad-widgets = { path = "../widgets", version="1.0.0", default-features = false}

[features]
default = ["bundled-fonts"]
bundled-fonts = ["makepad-widgets/bundled-fonts"]
system-fonts = ["makepad-widgets/system-fonts"]
```

### 4. libs/math_widget/Cargo.toml

Added feature flag propagation:

```toml
[dependencies]
makepad-widgets = { path = "../../widgets", default-features = false }

[features]
default = ["bundled-fonts"]
bundled-fonts = ["makepad-widgets/bundled-fonts"]
system-fonts = ["makepad-widgets/system-fonts"]
```

### 5. Theme Files (Desktop)

Removed Chinese and emoji font references from `font_family` definitions in:
- `widgets/src/theme_desktop_dark.rs`
- `widgets/src/theme_desktop_light.rs`
- `widgets/src/theme_desktop_skeleton.rs`

Before:
```rust
pub THEME_FONT_REGULAR = {
    font_family: {
        latin = font("crate://self/resources/IBMPlexSans-Text.ttf", -0.1, 0.0),
        chinese = font(
            "crate://makepad_fonts_chinese_regular/resources/LXGWWenKaiRegular.ttf",
            "crate://makepad_fonts_chinese_regular_2/resources/LXGWWenKaiRegular.ttf.2",
            0.0, 0.0)
        emoji = font("crate://makepad_fonts_emoji/resources/NotoColorEmoji.ttf", 0.0, 0.0)
    },
    line_spacing: 1.2
}
```

After:
```rust
pub THEME_FONT_REGULAR = {
    font_family: {
        latin = font("crate://self/resources/IBMPlexSans-Text.ttf", -0.1, 0.0),
    },
    line_spacing: 1.2
}
```

## Usage

### Using System Fonts in Your App

```toml
# In your Cargo.toml
[dependencies]
makepad-widgets = { version = "1.0", default-features = false, features = ["system-fonts"] }
makepad-code-editor = { version = "1.0", default-features = false, features = ["system-fonts"] }
```

### Using Bundled Fonts (Default)

```toml
# In your Cargo.toml (default behavior)
[dependencies]
makepad-widgets = "1.0"
```

### Running Examples

```bash
# Default (bundled fonts)
cargo run -p makepad-example-ui-zoo --release

# System fonts only
cargo run -p makepad-example-ui-zoo --no-default-features --features system-fonts --release
```

## Trade-offs

### Bundled Fonts

**Pros:**
- Consistent appearance across all platforms
- Guaranteed font availability
- Predictable text layout and metrics
- Works offline/embedded

**Cons:**
- Larger distribution size (~80MB for Chinese + emoji fonts)
- Memory overhead for font data
- Limited to bundled scripts only

### System Fonts

**Pros:**
- Smaller distribution size
- Native platform appearance
- Full Unicode coverage via OS fallback
- Lower memory usage

**Cons:**
- Platform-dependent rendering
- Potential layout differences across platforms
- Dependent on OS font availability

## Files Modified

| File | Change |
|------|--------|
| `widgets/Cargo.toml` | Made font crates optional |
| `widgets/src/lib.rs` | Conditional font initialization |
| `code_editor/Cargo.toml` | Feature flag propagation |
| `libs/math_widget/Cargo.toml` | Feature flag propagation |
| `widgets/src/theme_desktop_dark.rs` | Removed chinese/emoji font refs |
| `widgets/src/theme_desktop_light.rs` | Removed chinese/emoji font refs |
| `widgets/src/theme_desktop_skeleton.rs` | Removed chinese/emoji font refs |
| `draw/src/shader/draw_text.rs` | CJK fallback injection with platform constants |
| `draw/src/text/builtins.rs` | Programmatic font definitions with platform tuple constants |
| `draw/src/text/loader.rs` | Scoped `SystemFontProvider` import |
| `platform/src/os/windows/system_fonts.rs` | Fixed trait interface, simplified error handling |
| `platform/src/os/mod.rs` | Removed dead code |

## CJK Font Fallback

When using `system-fonts`, CJK (Chinese/Japanese/Korean) text rendering requires special handling due to platform-specific font availability.

### Automatic CJK Fallback Injection

The system automatically adds CJK fallback fonts to all DSL-defined font families when `system-fonts` is enabled. This happens in `draw/src/shader/draw_text.rs` during font family initialization using consolidated platform constants:

```rust
#[cfg(feature = "system-fonts")]
{
    #[cfg(target_os = "macos")]
    const CJK_FONT_NAME: &str = "STHeiti";
    #[cfg(target_os = "windows")]
    const CJK_FONT_NAME: &str = "Microsoft YaHei";
    #[cfg(target_os = "linux")]
    const CJK_FONT_NAME: &str = "Noto Sans CJK SC";

    let cjk_font_id: FontId = "SystemCJKFallback".into();
    if !fonts.is_font_known(cjk_font_id) {
        fonts.define_font(cjk_font_id, FontDefinition::from_system(CJK_FONT_NAME));
    }
    font_ids.push(cjk_font_id);
}
```

### Platform-Specific CJK Fonts

| Platform | CJK Font | Notes |
|----------|----------|-------|
| macOS | STHeiti | PingFang SC is a stub font (see below) |
| Windows | Microsoft YaHei | Standard CJK font |
| Linux | Noto Sans CJK SC | Requires font package installation |

### macOS Font Limitations

**Important:** Modern macOS system fonts like PingFang SC are "stub fonts" - they contain no glyph outlines (`glyf`, `CFF`, or `CFF2` tables). macOS renders these fonts through private APIs that third-party apps cannot access.

**Symptoms of stub font usage:**
- Text appears as empty spaces (not boxes)
- Glyph IDs are found by the shaper but rasterization fails
- Font file loads successfully but has no outline tables

**Solution:** Use older fonts with proper TrueType outlines:
- STHeiti (CJK) - Has `glyf` table
- Hiragino Sans (Japanese) - Has outlines
- Songti (Chinese) - Has `glyf` table

**Fonts to avoid on macOS:**
- PingFang SC/TC/HK - Stub fonts
- SF Pro - Stub font
- Apple system fonts prefixed with "." - Internal stub fonts

### Verifying Font Outlines

To check if a font has proper outlines:

```python
from fontTools import ttLib

font = ttLib.TTFont("/path/to/font.ttf", fontNumber=0)
has_outlines = 'glyf' in font or 'CFF ' in font or 'CFF2' in font
print(f"Has outlines: {has_outlines}")
print(f"Tables: {list(font.keys())}")
```

## Integration Example: moly-ai

The moly-ai project uses makepad-fonts with system fonts:

```toml
# moly-ai/Cargo.toml
[dependencies]
makepad-widgets = { path = "../makepad-fonts/widgets", features = ["system-fonts"], default-features = false }
makepad-code-editor = { path = "../makepad-fonts/code_editor", features = ["system-fonts"], default-features = false }

# Patch other dependencies that use makepad
[patch.'https://github.com/wyeworks/makepad']
makepad-widgets = { path = "../makepad-fonts/widgets" }
makepad-code-editor = { path = "../makepad-fonts/code_editor" }
makepad-draw = { path = "../makepad-fonts/draw" }
makepad-platform = { path = "../makepad-fonts/platform" }
math_widget = { path = "../makepad-fonts/libs/math_widget" }
```

## Troubleshooting

### CJK Text Shows as Empty Spaces

**Cause:** The system font provider loaded a stub font without glyph outlines.

**Solution:**
1. Check which font is being loaded (add debug print to `system_fonts.rs`)
2. Verify the font has outline tables using fontTools
3. Switch to a font with proper outlines (STHeiti on macOS)

### CJK Text Shows as Boxes (Tofu)

**Cause:** No CJK fallback font is available or the fallback doesn't cover the character.

**Solution:**
1. Verify CJK font is installed on the system
2. Check that `system-fonts` feature is enabled
3. Ensure the automatic fallback injection is working

### Font Loading Errors

**Cause:** System font provider cannot find or read the font file.

**Solution:**
1. Verify font is installed: `system_profiler SPFontsDataType | grep FontName`
2. Check font file permissions
3. On Linux, ensure fontconfig is installed and configured

## Code Patterns

### Platform Constant Pattern

To avoid code duplication across platforms, both `draw_text.rs` and `builtins.rs` use platform-specific constants:

**Single constant (draw_text.rs):**
```rust
#[cfg(target_os = "macos")]
const CJK_FONT_NAME: &str = "STHeiti";
#[cfg(target_os = "windows")]
const CJK_FONT_NAME: &str = "Microsoft YaHei";
#[cfg(target_os = "linux")]
const CJK_FONT_NAME: &str = "Noto Sans CJK SC";

// Single block of code uses CJK_FONT_NAME
```

**Tuple constant (builtins.rs):**
```rust
// (sans_font, cjk_font, mono_font)
#[cfg(target_os = "macos")]
const FONTS: (&str, &str, &str) = ("Helvetica Neue", "STHeiti", "Menlo");
#[cfg(target_os = "windows")]
const FONTS: (&str, &str, &str) = ("Segoe UI", "Microsoft YaHei", "Consolas");
#[cfg(target_os = "linux")]
const FONTS: (&str, &str, &str) = ("DejaVu Sans", "Noto Sans CJK SC", "DejaVu Sans Mono");

let (sans_font, cjk_font, mono_font) = FONTS;
// Single block of font registration code
```

This pattern reduces code duplication from ~3x (one block per platform) to a single implementation with platform-specific constants.
