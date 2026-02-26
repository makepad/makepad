use crate::window::{WindowIcon, WindowIconBuffer};
use makepad_zune_png::makepad_zune_core::bytestream::ZCursor;
use makepad_zune_png::PngDecoder;

include!(concat!(env!("OUT_DIR"), "/app_icon_gen.rs"));

pub fn window_icon() -> WindowIcon {
    let icon32 = decode_png(CUSTOM_ICON_PNG_32, 1);
    let icon64 = decode_png(CUSTOM_ICON_PNG_64, 1);
    let icon128 = decode_png(CUSTOM_ICON_PNG_128, 2);
    let icon256 = decode_png(CUSTOM_ICON_PNG_256, 2);
    let icon512 = decode_png(CUSTOM_ICON_PNG_512, 4);
    let icon1024 = decode_png(CUSTOM_ICON_PNG_1024, 8);

    #[cfg(target_os = "windows")]
    {
        let buffers: Vec<_> = [icon32, icon64, icon128, icon256, icon512, icon1024]
            .into_iter()
            .flatten()
            .collect();
        if !buffers.is_empty() {
            return WindowIcon {
                name: None,
                buffers,
            };
        }
    }

    #[cfg(target_os = "macos")]
    if let Some(buf) = icon1024
        .or(icon512)
        .or(icon256)
        .or(icon128)
        .or(icon64)
        .or(icon32)
    {
        return WindowIcon {
            name: None,
            buffers: vec![buf],
        };
    }

    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    if let Some(buf) = icon256
        .or(icon128)
        .or(icon64)
        .or(icon512)
        .or(icon1024)
        .or(icon32)
    {
        return WindowIcon {
            name: None,
            buffers: vec![buf],
        };
    }

    builtin_makepad_icon()
}

fn decode_png(png: &[u8], scale: u32) -> Option<WindowIconBuffer> {
    if png.is_empty() {
        return None;
    }
    let mut dec = PngDecoder::new(ZCursor::new(png));
    dec.decode_headers().ok()?;
    let (width, height) = dec.dimensions()?;
    let cs = dec.colorspace()?;
    let decoded = dec.decode().ok()?;
    let src = decoded.u8()?;

    let mut rgba = Vec::with_capacity(width * height * 4);
    match cs.num_components() {
        4 => rgba.extend_from_slice(&src),
        3 => {
            for c in src.chunks_exact(3) {
                rgba.extend_from_slice(&[c[0], c[1], c[2], 255]);
            }
        }
        2 => {
            for c in src.chunks_exact(2) {
                rgba.extend_from_slice(&[c[0], c[0], c[0], c[1]]);
            }
        }
        1 => {
            for g in src {
                rgba.extend_from_slice(&[g, g, g, 255]);
            }
        }
        _ => return None,
    }

    Some(WindowIconBuffer {
        width: width as u32,
        height: height as u32,
        scale: scale as i32,
        data: rgba,
    })
}

fn builtin_makepad_icon() -> WindowIcon {
    const SIZE: u32 = 64;
    let mut data = vec![0u8; (SIZE * SIZE * 4) as usize];

    for y in 0..SIZE {
        for x in 0..SIZE {
            let offset = ((y * SIZE + x) * 4) as usize;
            let cx = (x as f32) - 31.5;
            let cy = (y as f32) - 31.5;
            let radius = 12.0f32;
            let half = 31.5f32;
            let dx = (cx.abs() - (half - radius)).max(0.0);
            let dy = (cy.abs() - (half - radius)).max(0.0);
            let inside = (dx * dx + dy * dy) <= radius * radius;
            if inside {
                data[offset] = 0x2a;
                data[offset + 1] = 0x2a;
                data[offset + 2] = 0x3a;
                data[offset + 3] = 0xff;
            }
        }
    }

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

    for y in 16..48 {
        for x in 16..20 {
            draw_pixel(&mut data, x, y);
        }
        for x in 44..48 {
            draw_pixel(&mut data, x, y);
        }
    }
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
