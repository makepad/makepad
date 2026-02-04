// Match harfbuzz code style.
#![allow(non_camel_case_types)]
#![allow(non_upper_case_globals)]
#![allow(non_snake_case)]

mod algs;
#[macro_use]
pub mod buffer;
mod aat_layout;
mod aat_layout_kerx_table;
mod aat_layout_morx_table;
mod aat_layout_trak_table;
mod aat_map;
pub mod common;
pub mod face;
mod kerning;
mod machine_cursor;
mod ot;
mod ot_layout;
mod ot_layout_common;
mod ot_layout_gpos_table;
mod ot_layout_gsub_table;
mod ot_layout_gsubgpos;
mod ot_map;
mod ot_shape;
mod ot_shape_fallback;
mod ot_shape_normalize;
pub mod ot_shape_plan;
mod ot_shaper;
mod ot_shaper_arabic;
mod ot_shaper_arabic_table;
mod ot_shaper_hangul;
mod ot_shaper_hebrew;
mod ot_shaper_indic;
mod ot_shaper_indic_machine;
#[rustfmt::skip]
mod ot_shaper_indic_table;
mod ot_shaper_khmer;
mod ot_shaper_khmer_machine;
mod ot_shaper_myanmar;
mod ot_shaper_myanmar_machine;
mod ot_shaper_syllabic;
mod ot_shaper_thai;
mod ot_shaper_use;
mod ot_shaper_use_machine;
#[rustfmt::skip]
mod ot_shaper_use_table;
mod aat_layout_common;
mod ot_shaper_vowel_constraints;
mod paint_extents;
mod set_digest;
pub mod shape;
#[cfg(feature = "wasm-shaper")]
mod shape_wasm;
mod tag;
mod tag_table;
mod text_parser;
mod unicode;
mod unicode_norm;

use ttf_parser::Tag as hb_tag_t;

use self::buffer::hb_glyph_info_t;
use self::face::hb_font_t;

type hb_mask_t = u32;

use self::common::{script, Direction, Feature, Language, Script};
