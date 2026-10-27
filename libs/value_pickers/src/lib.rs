//! Visual editors for values written in code, as widgets a host places
//! where a property's value is a colour, and the panel kit (`panel`): the
//! controls a document's editor panel is made of.
//!
//! - [`color_picker::HdrColorPicker`]: a saturation/value plane with hue and
//!   alpha strips, the colour written down, an HDR intensity ("glow" above
//!   1), the file's palette with how many places use each colour, and
//!   whether an edit is this place's or every place's that colour (or tag)
//!   is written.
//!
//! It writes nothing: it says what is previewed (hovered, dragged)
//! and what is committed (clicked, let go), and the host maps those onto
//! its running document and its source (one undo step per commit).

pub mod color_picker;
pub mod panel;

pub use color_picker::{format_code_color, parse_code_color, split_hdr, ColorPickAction, HdrColorPicker, HdrColorPickerRef, PaletteEntry};

use makepad_widgets::*;

/// Register the pickers (after the widgets and Stage's `mod.stage` palette).
pub fn script_mod(vm: &mut ScriptVm) {
    color_picker::script_mod(vm);
    panel::script_mod(vm);
}
