//! The text stack's part of a collect run (`makepad_platform::collect`):
//! every glyph a layout returns is recorded per font file while the app
//! runs, and text a widget can show without drawing it now is laid out or
//! declared through [`DrawText::scan_text`] and [`DrawText::scan_open_text`].

use {
    crate::{
        makepad_platform::{collect::font_hash, Cx, ScanEvent, Scanner},
        shader::draw_text::DrawText,
        text::{font::FontId, fonts::Fonts},
    },
    std::{cell::RefCell, collections::HashMap, rc::Rc},
};

/// Moves the layouter's glyph record into the scan after every frame.
#[derive(Default)]
pub(crate) struct TextScanner {
    /// Font file hashes by font, computed once per font.
    hashes: HashMap<FontId, String>,
}

impl TextScanner {
    fn take(&mut self, cx: &mut Cx, scan: &ScanEvent) {
        let Some(fonts) = cx.get_global_ref::<Rc<RefCell<Fonts>>>().cloned() else { return };
        let recorded = fonts.borrow_mut().take_recorded_glyphs();
        for r in recorded {
            let hash = self.hashes.entry(r.font.id()).or_insert_with(|| font_hash(r.font.data().as_slice())).clone();
            scan.add_glyphs(&hash, r.glyphs, r.texts);
        }
    }
}

impl Scanner for TextScanner {
    fn name(&self) -> &str {
        "text"
    }

    fn step(&mut self, cx: &mut Cx, scan: &ScanEvent) -> bool {
        self.take(cx, scan);
        true
    }

    fn finish(&mut self, cx: &mut Cx, scan: &ScanEvent) {
        self.take(cx, scan);
        if let Some(fonts) = cx.get_global_ref::<Rc<RefCell<Fonts>>>() {
            scan.summary("missing_glyphs", fonts.borrow().missing_glyphs());
        }
    }
}

/// Turns the glyph record on and registers the scanner (collect runs only).
pub(crate) fn start(cx: &mut Cx, fonts: &mut Fonts) {
    if cx.is_collecting() {
        fonts.record_glyphs(true);
        cx.add_scanner(Box::new(TextScanner::default()));
    }
}

impl DrawText {
    /// Lays `text` out in this style without drawing it, so a scan records
    /// its glyphs: what a widget calls for text it can show but has not
    /// drawn (a closed menu's items, a template's labels).
    pub fn scan_text(&self, cx: &mut Cx, text: &str) {
        if !text.is_empty() {
            self.layout(cx, 0.0, 0.0, None, false, Default::default(), text);
        }
    }

    /// Text in this style cannot be bounded (what a person types, data):
    /// every font of its family ships whole.
    #[track_caller]
    pub fn scan_open_text(&self, cx: &mut Cx, scan: &ScanEvent) {
        self.text_style.ensure_fonts_loaded(cx);
        let Some(fonts) = cx.get_global_ref::<Rc<RefCell<Fonts>>>().cloned() else { return };
        let family = fonts.borrow_mut().get_or_load_font_family(self.text_style.font_family_id());
        for font in family.fonts() {
            scan.need_font_full(&font_hash(font.data().as_slice()));
        }
    }

    /// The characters of `chars` in every font of this style's family
    /// (text known to be drawn from a set: digits, a name list).
    #[track_caller]
    pub fn scan_chars(&self, cx: &mut Cx, scan: &ScanEvent, chars: &str) {
        self.text_style.ensure_fonts_loaded(cx);
        let Some(fonts) = cx.get_global_ref::<Rc<RefCell<Fonts>>>().cloned() else { return };
        let family = fonts.borrow_mut().get_or_load_font_family(self.text_style.font_family_id());
        for font in family.fonts() {
            scan.need_glyphs(&font_hash(font.data().as_slice()), chars);
        }
    }
}
