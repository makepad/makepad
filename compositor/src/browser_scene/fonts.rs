use std::cell::RefCell;
use std::rc::Rc;

use makepad_draw::text::font::{Font, FontId};
use makepad_draw::text::font_face::CanonicalVariations;
use makepad_draw::text::font_family::FontFamilyId;
use makepad_draw::text::loader::{FontDefinition, FontFamilyDefinition};
use makepad_draw::SharedBytes;

use super::MpBrowserFontResource;
use crate::*;

fn ensure_font_registered_with_fonts(
    fonts: &mut makepad_draw::text::fonts::Fonts,
    font: &MpBrowserFontResource,
) -> FontId {
    let font_id = FontId::from(font.key);
    if !fonts.is_font_known(font_id) {
        fonts.define_font(
            font_id,
            FontDefinition {
                data: SharedBytes::from_arc(font.bytes.clone()),
                index: font.face_index,
                ascender_fudge_in_ems: 0.0,
                descender_fudge_in_ems: 0.0,
                variations: CanonicalVariations::default(),
            },
        );
    }
    font_id
}

pub(super) fn ensure_font_family(
    fonts: &mut makepad_draw::text::fonts::Fonts,
    font_id: FontId,
) -> FontFamilyId {
    use std::hash::{Hash, Hasher};

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    font_id.hash(&mut hasher);
    let family_id = FontFamilyId::from(hasher.finish());
    if !fonts.is_font_family_known(family_id) {
        fonts.define_font_family(
            family_id,
            FontFamilyDefinition {
                font_ids: vec![font_id],
                expected_member_count: 1,
            },
        );
    }
    family_id
}

pub(super) fn resolve_font_with_fonts(
    fonts_rc: &Rc<RefCell<makepad_draw::text::fonts::Fonts>>,
    font: &MpBrowserFontResource,
) -> Option<Rc<Font>> {
    let mut fonts = fonts_rc.borrow_mut();
    let font_id = ensure_font_registered_with_fonts(&mut fonts, font);
    let family_id = ensure_font_family(&mut fonts, font_id);
    let family = fonts.get_or_load_font_family(family_id);
    family.fonts().first().cloned()
}
