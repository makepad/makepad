//! Scalefactor bands of a granule in bitstream order: 22 long bands, or 13
//! short bands times three windows (band-major, window-minor), the same
//! layout the decoder's `BandLayout` walks.

use makepad_audio_decode::mp3::tables::{SFB_LONG_MPEG1, SFB_SHORT_MPEG1};

#[derive(Clone)]
pub struct Bands {
    pub short: bool,
    pub count: usize,
    pub start: [u16; 39],
    pub width: [u16; 39],
    /// The band's transmitted scalefactor, `None` for the top band (MPEG-1
    /// sends 21 long or 12 short ones).
    pub has_sf: [bool; 39],
    /// Short window of the band (0 for long bands).
    pub window: [u8; 39],
    /// First line of the band within its window's spectrum.
    pub line_in_window: [u16; 39],
    /// Long-block band boundaries, for region selection.
    pub sfb_long: [u16; 23],
}

/// Index into the MPEG-1 tables: 44.1, 48, 32 kHz.
pub fn rate_index(rate: u32) -> usize {
    match rate {
        44_100 => 0,
        48_000 => 1,
        _ => 2,
    }
}

impl Bands {
    pub fn new(rate: u32, short: bool) -> Self {
        let ri = rate_index(rate);
        let long = &SFB_LONG_MPEG1[ri];
        let mut b = Bands {
            short,
            count: 0,
            start: [0; 39],
            width: [0; 39],
            has_sf: [false; 39],
            window: [0; 39],
            line_in_window: [0; 39],
            sfb_long: *long,
        };
        if !short {
            for i in 0..22 {
                b.start[i] = long[i];
                b.width[i] = long[i + 1] - long[i];
                b.has_sf[i] = i < 21;
                b.line_in_window[i] = long[i];
            }
            b.count = 22;
        } else {
            let s = &SFB_SHORT_MPEG1[ri];
            let mut line = 0u16;
            for sfb in 0..13 {
                let width = s[sfb + 1] - s[sfb];
                for w in 0..3 {
                    let e = 3 * sfb + w;
                    b.start[e] = line;
                    b.width[e] = width;
                    b.has_sf[e] = sfb < 12;
                    b.window[e] = w as u8;
                    b.line_in_window[e] = s[sfb];
                    line += width;
                }
            }
            b.count = 39;
        }
        b
    }
}
