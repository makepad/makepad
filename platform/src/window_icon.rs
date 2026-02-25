use std::sync::OnceLock;
use crate::window::{WindowIcon, WindowIconBuffer};

/// Global icon override. When set, `default_window_icon()` returns this
/// instead of the built-in "M" icon.
static GLOBAL_ICON: OnceLock<WindowIcon> = OnceLock::new();

/// Set a global window icon that all new windows will use.
/// Must be called before the first window is created.
pub fn set_window_icon(icon: WindowIcon) {
    let _ = GLOBAL_ICON.set(icon);
}

/// Return the global icon override if set, otherwise the built-in "M" icon.
pub fn default_window_icon() -> WindowIcon {
    if let Some(icon) = GLOBAL_ICON.get() {
        return icon.clone();
    }
    default_makepad_icon()
}

/// Generate the default Makepad window icon (64x64 RGBA8).
/// A simple "M" glyph on a dark background.
fn default_makepad_icon() -> WindowIcon {
    const SIZE: u32 = 64;
    let mut data = vec![0u8; (SIZE * SIZE * 4) as usize];

    // Dark background with rounded-corner feel
    for y in 0..SIZE {
        for x in 0..SIZE {
            let offset = ((y * SIZE + x) * 4) as usize;
            // Simple rounded rect: corners outside radius are transparent
            let cx = (x as f32) - 31.5;
            let cy = (y as f32) - 31.5;
            let radius = 12.0f32;
            let half = 31.5f32;
            let dx = (cx.abs() - (half - radius)).max(0.0);
            let dy = (cy.abs() - (half - radius)).max(0.0);
            let inside = (dx * dx + dy * dy) <= radius * radius;
            if inside {
                // Background: dark blue-grey (#2a2a3a)
                data[offset] = 0x2a;
                data[offset + 1] = 0x2a;
                data[offset + 2] = 0x3a;
                data[offset + 3] = 0xff;
            }
        }
    }

    // Draw "M" in white-ish color (#e0e0f0)
    let draw_pixel = |data: &mut Vec<u8>, x: i32, y: i32| {
        if x >= 0 && x < SIZE as i32 && y >= 0 && y < SIZE as i32 {
            let offset = ((y as u32 * SIZE + x as u32) * 4) as usize;
            if data[offset + 3] == 0xff {
                data[offset] = 0xe0;
                data[offset + 1] = 0xe0;
                data[offset + 2] = 0xf0;
            }
        }
    };

    // M shape: two verticals + two diagonals meeting at center
    // Left vertical: x=16..20, y=16..48
    // Right vertical: x=44..48, y=16..48
    // Left diagonal: from (16,16) to (32,32)
    // Right diagonal: from (48,16) to (32,32)
    for y in 16..48 {
        for x in 16..20 {
            draw_pixel(&mut data, x, y);
        }
        for x in 44..48 {
            draw_pixel(&mut data, x, y);
        }
    }
    // Diagonals
    for i in 0..16 {
        let lx = 18 + i;
        let rx = 46 - i;
        let y = 16 + i;
        for dx in -1..=1 {
            draw_pixel(&mut data, lx + dx, y);
            draw_pixel(&mut data, rx + dx, y);
        }
    }

    WindowIcon {
        name: None,
        buffers: vec![WindowIconBuffer {
            width: SIZE,
            height: SIZE,
            scale: 1,
            data,
        }],
    }
}
