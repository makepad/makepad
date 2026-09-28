//! Charts, the colour picker family, dates, the dropzone, chat, glass panels and the vector widget: small families that share one crate.
//!
//! A family crate of `makepad-widgets`: it builds on `makepad-widgets-core`
//! alone, has no features of its own, and compiles once whatever mix of
//! families an app picks. `makepad-widgets` re-exports it under the module
//! paths it always had and registers it when the app's features ask for it.

// The core's modules and prelude, as `crate::...` paths for the modules
// that moved here from it.
use makepad_widgets_core::*;

pub mod calendar;
pub mod chart;
pub mod chart_shapes;
pub mod chat;
pub mod color;
pub mod date_picker;
pub mod dropzone;
pub mod glass_panel;
pub mod gradient_editor;
pub mod time_picker;
pub mod vector;
pub mod waveform;

/// Registers the glass panel family.
pub fn glass_mod(vm: &mut ScriptVm) {
    crate::glass_panel::script_mod(vm);
}

/// Registers the calendar, the date picker and the time picker.
pub fn dates_mod(vm: &mut ScriptVm) {
    crate::calendar::script_mod(vm);
    crate::date_picker::script_mod(vm);
    crate::time_picker::script_mod(vm);
}

/// Registers the colour picker family and the gradient editor built on it.
pub fn color_mod(vm: &mut ScriptVm) {
    crate::color::script_mod(vm);
    // After the colour picker, whose button and popup it stops with.
    crate::gradient_editor::script_mod(vm);
}

/// Registers the waveform lane, the chart shapes and the charts.
pub fn charts_mod(vm: &mut ScriptVm) {
    crate::waveform::script_mod(vm);
    crate::chart_shapes::script_mod(vm);
    crate::chart::script_mod(vm);
}

/// Registers the chat bubbles and the chat list.
pub fn chat_mod(vm: &mut ScriptVm) {
    crate::chat::script_mod(vm);
}

/// Registers the file drop target and the upload list.
pub fn dropzone_mod(vm: &mut ScriptVm) {
    crate::dropzone::script_mod(vm);
}

/// Registers the vector drawing widget.
pub fn vector_mod(vm: &mut ScriptVm) {
    crate::vector::script_mod(vm);
}

// The pooled test contexts and the whole registration the tests build on:
// the core with every family in this crate.
#[cfg(test)]
pub(crate) use makepad_widgets_core::test_cx::PooledCx;

#[cfg(test)]
pub(crate) fn script_mod(vm: &mut ScriptVm) {
    makepad_widgets_core::script_mod_with(
        vm,
        WindowFamilies::default(),
        &[glass_mod, dates_mod, color_mod, charts_mod, chat_mod, dropzone_mod, vector_mod],
    );
}

#[cfg(test)]
pub(crate) fn checkout_test_cx() -> PooledCx {
    makepad_widgets_core::test_cx::checkout_test_cx(script_mod)
}

#[cfg(test)]
mod field_row_tests {
    use makepad_widgets_core::desktop_style::{install, DesktopStyle, StyleSheet};
    use makepad_widgets_core::makepad_script::trap::NoTrap;
    use makepad_widgets_core::*;

    /// A field's box metrics as the sheet leaves them: the minimum height,
    /// and the padding above and below the line.
    fn field_metrics(vm: &mut ScriptVm, name: &str) -> (Option<f64>, Option<f64>, Option<f64>) {
        let widgets = vm.module(id!(widgets));
        let widget = vm
            .bx
            .heap
            .value(widgets, LiveId::from_str(name).into(), NoTrap)
            .as_object()
            .unwrap_or_else(|| panic!("the family has no widget named {name}"));
        let min = vm.bx.heap.value(widget, id!(min_height).into(), NoTrap).as_f64();
        let pad = vm.bx.heap.value(widget, id!(padding).into(), NoTrap).as_object();
        let side = |vm: &mut ScriptVm, key: LiveId| pad.and_then(|p| vm.bx.heap.value(p, key.into(), NoTrap).as_f64());
        (min, side(vm, id!(top)), side(vm, id!(bottom)))
    }

    /// The date and time fields stand in the row a sheet's text box sets,
    /// and lay their own parts out, so no vertical padding moves the line
    /// off the box: the core holds its own fields to the same row.
    #[test]
    fn the_date_fields_stand_at_the_height_their_sheet_gives_the_text_box() {
        for (style, dark) in DesktopStyle::ALL
            .into_iter()
            .flat_map(|style| if style.supports_dark() { vec![(style, false), (style, true)] } else { vec![(style, false)] })
        {
            let mut cx = Cx::new(Box::new(|_, _| {}));
            cx.with_vm(|vm| {
                crate::script_mod(vm);
                install(vm, StyleSheet::load_with_appearance(style, dark));
                vm.bx.captured_errors = Some(Vec::new());
                vm.with_reload(crate::script_mod);
                let sheet = if dark { format!("{}-dark", style.id()) } else { style.id().to_string() };
                assert!(vm.take_errors().is_empty(), "{sheet} does not evaluate");
                let row = field_metrics(vm, "TextInput");
                for name in ["DateField", "TimeField", "DatePicker", "DateRangePicker"] {
                    let field = field_metrics(vm, name);
                    assert_eq!(field.0, row.0, "{sheet}: {name} does not stand at the height of the row");
                    assert!(
                        field.1.unwrap_or(0.0) == 0.0 && field.2.unwrap_or(0.0) == 0.0,
                        "{sheet}: {name} lays its own parts out, so a vertical padding on it moves the line off the box"
                    );
                }
            });
        }
    }
}

#[cfg(test)]
pub(crate) fn on_test_cx(f: impl FnOnce() + Send + 'static) {
    makepad_widgets_core::test_cx::on_test_cx(f)
}
