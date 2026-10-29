//! Native content lives independently of its placement. Commands change its content; each
//! window's frame packet supplies its geometry and draw order through [`crate::Layer`].

pub mod kind {
    pub const WEB: u32 = 1;
    pub const CAMERA_PREVIEW: u32 = 2;
    /// UTF-8 registered WM app name, never a command line.
    pub const FOREIGN_WAYLAND: u32 = 3;
}

pub mod op {
    /// UTF-8 URL in `data`; `arg != 0` replaces the history entry.
    pub const NAVIGATE: u32 = 1;
    /// Signed i32 history delta in the low 32 bits of `arg`.
    pub const HISTORY: u32 = 2;
    pub const RELOAD: u32 = 3;
    pub const STOP: u32 = 4;
    /// `arg & 1`: dark appearance; four little-endian f32 background RGBA values in `data`.
    pub const APPEARANCE: u32 = 5;
    pub const FOCUS: u32 = 6;
    pub const BLUR: u32 = 7;
    /// An input event encoded with [`crate::buffers::pack_forward`].
    pub const INPUT: u32 = 8;
    /// Input id in `arg`, little-endian u64 format id in `data`.
    pub const CAMERA_SOURCE: u32 = 9;
    /// Physical content width and height as two little-endian u32 values.
    pub const SIZE: u32 = 10;
}

pub mod mode {
    pub const UNSUPPORTED: u64 = 0;
    pub const COMPOSITED: u64 = 1;
    pub const TEXTURED: u64 = 2;
}

pub mod state {
    /// `a`: [`super::mode`]; TEXTURED `b`: lent texture alias index in the low 32 bits,
    /// generation in the high 32 bits.
    pub const REALIZED: u32 = 1;
    pub const TITLE: u32 = 2;
    pub const URL: u32 = 3;
    pub const LOADING: u32 = 4;
    /// `a != 0`: can go back; `b != 0`: can go forward.
    pub const NAV: u32 = 5;
    pub const FOCUS_IN: u32 = 6;
    pub const FOCUS_OUT: u32 = 7;
    pub const CURSOR: u32 = 8;
    pub const CRASHED: u32 = 9;
    pub const CLOSED: u32 = 10;
    /// The texture backing changed; retained draws need another frame.
    pub const FRAME: u32 = 11;
}

pub mod layer_flags {
    pub const VISIBLE: u32 = 1;
    pub const INPUT_PASSTHROUGH: u32 = 2;
}
