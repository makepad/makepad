//! Extra widget style sheets, `themes/<id>/{theme,widgets}.splash`. They lay
//! out as tiles (the Omarchy family) and name their fonts from the widget
//! library's resources, so `crate_resource("makepad_widgets:...")` resolves
//! against `makepad_widgets_core::widgets_dir`.
use makepad_widgets_core::desktop_style::{self, DesktopStyle, SheetEntry};
use makepad_widgets_core::sheet_entry;

pub mod cap;

macro_rules! sheet {
    ($id:literal, $label:literal) => {
        sheet_entry!(
            dir: "themes", id: $id, label: $label, family: DesktopStyle::Omarchy,
            dark: None, resources: makepad_widgets_core::widgets_dir,
            script_mod: crate::script_mod,
        )
    };
}

/// Every sheet here.
pub static SHEETS: [SheetEntry; 11] = [
    sheet!("cyberpunk", "Cyberpunk"),
    sheet!("aluminium", "Aluminium"),
    sheet!("ferrofluid", "Ferrofluid"),
    sheet!("segment-display", "Segment display"),
    sheet!("lacquer-red", "Lacquer red"),
    sheet!("safety-yellow", "Safety yellow"),
    sheet!("field-radio", "Field radio"),
    sheet!("steampunk-brass", "Steampunk brass"),
    sheet!("futuristic-metal", "Futuristic metal"),
    sheet!("concrete", "Concrete"),
    sheet!("handheld", "Handheld"),
];

/// Add these sheets to the catalogue. Call it at startup, before the first
/// style load; calling it again changes nothing.
pub fn register() {
    desktop_style::register(&SHEETS);
}

/// Register the shader modules the sheets call (`mod.sdf.Cap`, the fader cap
/// engine). A sheet names them when it loads, so call it after the widget
/// library's `script_mod` and before the first style load. Other catalogues
/// whose sheets draw with the engine (Stage's) name this as their own module.
pub fn script_mod(vm: &mut makepad_widgets_core::ScriptVm) {
    cap::script_mod(vm);
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_widgets::*;
    use makepad_widgets_core::sheet_checks;

    fn full(vm: &mut ScriptVm) {
        register();
        makepad_widgets::script_mod(vm);
    }

    #[test]
    fn the_sheets_join_the_catalogue_once_after_the_librarys() {
        let before: Vec<&str> = desktop_style::catalogue().into_iter().map(|entry| entry.id).collect();
        register();
        register();
        let all = desktop_style::catalogue();
        let ids: Vec<&str> = all.iter().map(|entry| entry.id).collect();
        assert_eq!(&ids[..before.len()], &before[..], "the library's sheets stay first, in their order");
        for entry in &SHEETS {
            assert_eq!(ids.iter().filter(|id| **id == entry.id).count(), 1, "{} is not in the catalogue once", entry.id);
            assert!(desktop_style::find(entry.id).is_some_and(|found| std::ptr::eq(found, entry)));
        }
    }

    #[test]
    fn every_sheet_evaluates_and_names_its_base() {
        for entry in &SHEETS {
            sheet_checks::evaluates(full, entry);
        }
    }

    #[test]
    fn every_field_stands_at_the_height_its_sheet_gives_the_text_box() {
        for entry in &SHEETS {
            sheet_checks::field_row(full, entry);
        }
    }

    #[test]
    fn every_sheet_keeps_the_fallbacks_behind_its_own_face() {
        let checked: usize = SHEETS.iter().map(|entry| sheet_checks::font_fallbacks(full, entry)).sum();
        assert!(checked > 50, "only {checked} faces were read");
    }

    #[test]
    fn a_sheet_lays_the_window_ground_only_when_it_asks_for_one() {
        for entry in &SHEETS {
            sheet_checks::window_ground(full, entry);
        }
    }

    #[test]
    fn text_reads_on_its_ground_under_every_sheet() {
        for entry in &SHEETS {
            sheet_checks::contrast(full, entry);
        }
    }

    /// The corner each sheet gives its caps, as its token half sets it.
    #[test]
    fn every_sheet_sets_its_corner() {
        for (id, radius) in [
            ("cyberpunk", 2.5),
            ("aluminium", 2.0),
            ("ferrofluid", 2.5),
            ("segment-display", 1.5),
            ("lacquer-red", 3.0),
            ("safety-yellow", 2.0),
            ("field-radio", 2.0),
            ("steampunk-brass", 2.0),
            ("futuristic-metal", 2.0),
            ("concrete", 1.0),
            ("handheld", 0.0),
        ] {
            let entry = SHEETS.iter().find(|entry| entry.id == id).unwrap();
            assert_eq!(sheet_checks::theme_number(full, entry, "corner_radius"), Some(radius), "{id}");
        }
    }
}
