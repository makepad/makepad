//! Script glue for `KeyCode` (makepad-key-code, re-exported by the Studio
//! protocol), so `KeyCode.F10` and the other variants work in the DSL.
//!
//! The impls live here rather than as a `#[derive(Script)]` on the enum so that
//! makepad-studio-protocol does not depend on makepad-script: that dependency
//! made makepad-platform wait for the protocol crate, which waited for all of
//! script. They do what the derive does for a unit-only enum: each variant is
//! a frozen object in the `KeyCode` enum object, `script_new` picks `Escape`,
//! and the errors are the derive's.
use crate::apply::*;
use crate::heap::*;
use crate::traits::*;
use crate::value::*;
use crate::vm::*;
use crate::*;
use makepad_live_id::*;
use makepad_key_code::{KeyCode, KEYCODE_VARIANTS};

const KEY_CODE_COUNT: usize = KEYCODE_VARIANTS.len();

/// The variant ids, in `KEYCODE_VARIANTS` (declaration) order.
const KEY_CODE_IDS: [LiveId; KEY_CODE_COUNT] = {
    let mut ids = [LiveId(0); KEY_CODE_COUNT];
    let mut i = 0;
    while i < KEY_CODE_COUNT {
        ids[i] = LiveId::from_str(key_code_name(KEYCODE_VARIANTS[i]));
        i += 1;
    }
    ids
};

/// The variant's name; exhaustive, so a new variant has to be named here.
const fn key_code_name(key: KeyCode) -> &'static str {
    match key {
        KeyCode::Escape => "Escape",
        KeyCode::Back => "Back",
        KeyCode::Backtick => "Backtick",
        KeyCode::Key0 => "Key0",
        KeyCode::Key1 => "Key1",
        KeyCode::Key2 => "Key2",
        KeyCode::Key3 => "Key3",
        KeyCode::Key4 => "Key4",
        KeyCode::Key5 => "Key5",
        KeyCode::Key6 => "Key6",
        KeyCode::Key7 => "Key7",
        KeyCode::Key8 => "Key8",
        KeyCode::Key9 => "Key9",
        KeyCode::Minus => "Minus",
        KeyCode::Equals => "Equals",
        KeyCode::Backspace => "Backspace",
        KeyCode::Tab => "Tab",
        KeyCode::KeyQ => "KeyQ",
        KeyCode::KeyW => "KeyW",
        KeyCode::KeyE => "KeyE",
        KeyCode::KeyR => "KeyR",
        KeyCode::KeyT => "KeyT",
        KeyCode::KeyY => "KeyY",
        KeyCode::KeyU => "KeyU",
        KeyCode::KeyI => "KeyI",
        KeyCode::KeyO => "KeyO",
        KeyCode::KeyP => "KeyP",
        KeyCode::LBracket => "LBracket",
        KeyCode::RBracket => "RBracket",
        KeyCode::ReturnKey => "ReturnKey",
        KeyCode::KeyA => "KeyA",
        KeyCode::KeyS => "KeyS",
        KeyCode::KeyD => "KeyD",
        KeyCode::KeyF => "KeyF",
        KeyCode::KeyG => "KeyG",
        KeyCode::KeyH => "KeyH",
        KeyCode::KeyJ => "KeyJ",
        KeyCode::KeyK => "KeyK",
        KeyCode::KeyL => "KeyL",
        KeyCode::Semicolon => "Semicolon",
        KeyCode::Quote => "Quote",
        KeyCode::Backslash => "Backslash",
        KeyCode::KeyZ => "KeyZ",
        KeyCode::KeyX => "KeyX",
        KeyCode::KeyC => "KeyC",
        KeyCode::KeyV => "KeyV",
        KeyCode::KeyB => "KeyB",
        KeyCode::KeyN => "KeyN",
        KeyCode::KeyM => "KeyM",
        KeyCode::Comma => "Comma",
        KeyCode::Period => "Period",
        KeyCode::Slash => "Slash",
        KeyCode::Control => "Control",
        KeyCode::Alt => "Alt",
        KeyCode::Shift => "Shift",
        KeyCode::Logo => "Logo",
        KeyCode::Space => "Space",
        KeyCode::Capslock => "Capslock",
        KeyCode::F1 => "F1",
        KeyCode::F2 => "F2",
        KeyCode::F3 => "F3",
        KeyCode::F4 => "F4",
        KeyCode::F5 => "F5",
        KeyCode::F6 => "F6",
        KeyCode::F7 => "F7",
        KeyCode::F8 => "F8",
        KeyCode::F9 => "F9",
        KeyCode::F10 => "F10",
        KeyCode::F11 => "F11",
        KeyCode::F12 => "F12",
        KeyCode::PrintScreen => "PrintScreen",
        KeyCode::ScrollLock => "ScrollLock",
        KeyCode::Pause => "Pause",
        KeyCode::Insert => "Insert",
        KeyCode::Delete => "Delete",
        KeyCode::Home => "Home",
        KeyCode::End => "End",
        KeyCode::PageUp => "PageUp",
        KeyCode::PageDown => "PageDown",
        KeyCode::Numpad0 => "Numpad0",
        KeyCode::Numpad1 => "Numpad1",
        KeyCode::Numpad2 => "Numpad2",
        KeyCode::Numpad3 => "Numpad3",
        KeyCode::Numpad4 => "Numpad4",
        KeyCode::Numpad5 => "Numpad5",
        KeyCode::Numpad6 => "Numpad6",
        KeyCode::Numpad7 => "Numpad7",
        KeyCode::Numpad8 => "Numpad8",
        KeyCode::Numpad9 => "Numpad9",
        KeyCode::NumpadEquals => "NumpadEquals",
        KeyCode::NumpadSubtract => "NumpadSubtract",
        KeyCode::NumpadAdd => "NumpadAdd",
        KeyCode::NumpadDecimal => "NumpadDecimal",
        KeyCode::NumpadMultiply => "NumpadMultiply",
        KeyCode::NumpadDivide => "NumpadDivide",
        KeyCode::Numlock => "Numlock",
        KeyCode::NumpadEnter => "NumpadEnter",
        KeyCode::ArrowUp => "ArrowUp",
        KeyCode::ArrowDown => "ArrowDown",
        KeyCode::ArrowLeft => "ArrowLeft",
        KeyCode::ArrowRight => "ArrowRight",
        KeyCode::Unknown => "Unknown",
    }
}

fn key_code_from_id(id: LiveId) -> Option<KeyCode> {
    KEY_CODE_IDS
        .iter()
        .position(|variant| *variant == id)
        .map(|index| KEYCODE_VARIANTS[index])
}

fn key_code_error(vm: &mut ScriptVm, message: String) {
    if let ScriptTrap::Inner(trap) = vm.bx.threads.cur().trap.pass() {
        trap.push_err(
            ScriptValue::script_err_unknown_type(trap.ip),
            message,
            file!().into(),
            line!(),
        );
    }
}

impl ScriptDeriveMarker for KeyCode {}

impl ScriptHook for KeyCode {}

impl ScriptNew for KeyCode {
    fn script_type_id_static() -> ScriptTypeId {
        ScriptTypeId::of::<Self>()
    }

    fn script_type_name() -> Option<LiveId> {
        Some(LiveId::from_str_with_lut("KeyCode").unwrap())
    }

    fn script_new(_vm: &mut ScriptVm) -> Self {
        Self::Escape
    }

    fn script_default(vm: &mut ScriptVm) -> ScriptValue {
        Self::script_proto(vm);
        Self::script_new(vm).script_to_value(vm)
    }

    fn script_new_with_default(vm: &mut ScriptVm) -> Self {
        Self::script_new(vm)
    }

    fn script_reload_default(vm: &mut ScriptVm) -> ScriptValue {
        if vm
            .bx
            .heap
            .type_default_for_id(Self::script_type_id_static())
            .is_some()
        {
            Self::script_new_with_default(vm).script_to_value(vm)
        } else {
            NIL
        }
    }

    fn script_type_check(heap: &ScriptHeap, value: ScriptValue) -> bool {
        if <Self as ScriptHook>::on_type_check(heap, value) {
            return true;
        }
        if let Some(o) = value.as_object() {
            if let Some(id) = heap.root_proto(o).as_id() {
                return KEY_CODE_IDS.contains(&id);
            }
        }
        false
    }

    fn script_proto_build(vm: &mut ScriptVm, _props: &mut ScriptTypeProps) -> ScriptValue {
        let enum_object = vm.bx.heap.new_object();
        let enum_key = LiveId::from_str_with_lut("__enum").unwrap();
        let type_name = LiveId::from_str_with_lut("KeyCode").unwrap();
        for (key, id) in KEYCODE_VARIANTS.iter().zip(KEY_CODE_IDS) {
            let name = LiveId::from_str_with_lut(key_code_name(*key)).unwrap();
            let bare = vm.bx.heap.new_with_proto(name.into());
            vm.bx.heap.set_value(
                enum_object,
                id.into(),
                bare.into(),
                vm.bx.threads.cur().trap.pass(),
            );
            vm.bx.heap.set_value(
                bare,
                enum_key.into(),
                type_name.into(),
                vm.bx.threads.cur().trap.pass(),
            );
            vm.bx.heap.freeze(bare);
        }
        enum_object.into()
    }
}

impl ScriptApply for KeyCode {
    fn script_type_id(&self) -> ScriptTypeId {
        ScriptTypeId::of::<Self>()
    }

    fn script_apply(
        &mut self,
        vm: &mut ScriptVm,
        apply: &Apply,
        scope: &mut Scope,
        value: ScriptValue,
    ) {
        if self.on_custom_apply(vm, apply, scope, value) {
            return;
        }
        if let Some(object) = value.as_object() {
            if let Some(id) = vm.bx.heap.root_proto(object).as_id() {
                if let Some(key) = key_code_from_id(id) {
                    *self = key;
                    return;
                }
                let obj_desc = vm.format_object_for_error(object);
                key_code_error(
                    vm,
                    format!("unknown variant '{id}' for enum KeyCode, object: {obj_desc}"),
                );
                return;
            }
            let obj_desc = vm.format_object_for_error(object);
            key_code_error(
                vm,
                format!("expected variant id for enum KeyCode, got object: {obj_desc}"),
            );
            return;
        }
        let value_desc = vm.format_enum_variant_error(value);
        key_code_error(
            vm,
            format!("expected variant for enum KeyCode, got {value_desc}"),
        );
    }

    fn script_to_value(&self, vm: &mut ScriptVm) -> ScriptValue {
        if let Some(value) = <Self as ScriptHook>::on_custom_to_value(self, vm) {
            return value;
        }
        Self::script_enum_lookup_variant(vm, LiveId::from_str(key_code_name(*self)))
    }
}
