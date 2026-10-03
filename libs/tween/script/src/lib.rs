//! The tween eases as Splash natives in `mod.shared` (the shared stdlib
//! makepad-script-compute's `register_shared_std` registers):
//! `ease(name, u)`, an ease by name, `ease(@ease_out_cubic, prog(t, 1.0,
//! 1.6))`, the same table a Motion Tween's or key's `ease:` reads
//! (makepad-tween's `named::Ease`), so one name is one curve everywhere; f64,
//! `u` clamped to 0..1, one native call however many curves there are.
//! `ease_id(name)`: the ease's number in the kernels' `std.ease.apply_d`.
//!
//! Engine code, kept out of the Splash compute core so the kernel compiler
//! does not depend on the tween engine: hosts call [`install_ease`] after
//! `register_shared_std` (until then the two names raise an error naming
//! this function).

use makepad_script::*;
use makepad_tween::named::{Curve, Ease};

/// Installs `ease` and `ease_id` into `mod.shared`, replacing the
/// placeholders `register_shared_std` puts there. Panics when `mod.shared`
/// is not registered yet.
pub fn install_ease(vm: &mut ScriptVm) {
    let modules = vm.bx.heap.modules;
    let shared = vm
        .bx
        .heap
        .value(modules, id!(shared).into(), NoTrap)
        .as_object()
        .expect("install_ease: call makepad_script_compute::module::register_shared_std first");
    const CURVES: [Curve; 10] = [Curve::Quad, Curve::Cubic, Curve::Quart, Curve::Quint, Curve::Sine, Curve::Expo, Curve::Circ, Curve::Back, Curve::Elastic, Curve::Bounce];
    fn named(vm: &mut ScriptVm, name: ScriptValue) -> Result<Ease, ScriptValue> {
        let id = name.as_id().and_then(|id| id.as_string(|s| s.map(str::to_string)));
        match id.as_deref().and_then(Ease::from_id) {
            Some(ease) => Ok(ease),
            None => {
                let shown = id.map_or_else(|| "ease(name, u) takes an ease's @name first".to_string(), |n| format!("@{n} is not an ease"));
                Err(script_err_invalid_args!(
                    vm.bx.threads.cur_ref().trap,
                    "{shown}: @linear, @hold, @ease_in, @ease_out, @ease_in_out, @ease_{{in,out,in_out}}_{{quad,cubic,quart,quint,sine,expo,circ,back,elastic,bounce}}"
                ))
            }
        }
    }
    let curve = |c: Curve| CURVES.iter().position(|k| *k == c).unwrap_or(0);
    vm.add_method(shared, id_lut!(ease), script_args!(name = NIL, u = 0.0), |vm, args| {
        let name = vm.bx.heap.value(args, id!(name).into(), NoTrap);
        let u = vm.bx.heap.value(args, id!(u).into(), NoTrap).as_number().unwrap_or(0.0);
        match named(vm, name) {
            Ok(ease) => ease.apply(u, 0.0).into(),
            Err(e) => e,
        }
    });
    vm.add_method(shared, id_lut!(ease_id), script_args!(name = NIL), move |vm, args| {
        let name = vm.bx.heap.value(args, id!(name).into(), NoTrap);
        let id = match named(vm, name) {
            Ok(Ease::Linear) => 0,
            Ok(Ease::Hold) => 1,
            Ok(Ease::In(c)) => 2 + 3 * curve(c),
            Ok(Ease::Out(c)) => 3 + 3 * curve(c),
            Ok(Ease::InOut(c)) => 4 + 3 * curve(c),
            Ok(_) => return script_err_invalid_args!(vm.bx.threads.cur_ref().trap, "ease_id takes a named ease, not a spring or a bezier"),
            Err(e) => return e,
        };
        (id as f64).into()
    });
}

