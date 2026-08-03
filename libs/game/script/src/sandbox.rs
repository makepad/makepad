//! Capability stripping for downloaded games.
//!
//! game.md: *"Untrusted games are untrusted code. Online-shared `game.splash`
//! runs in our VM: shared-game isolates must be capability-stripped (no
//! `std.run`, no net, no fs beyond the game dir, instruction + heap budgets)."*
//!
//! A game authored by the kid at the keyboard is trusted — it can do whatever
//! the VM allows. A game that arrived from the registry or from a peer on the
//! LAN is not, and gets a `Trust::Downloaded` isolate.
//!
//! ## How the strip works
//!
//! `makepad_script_std::script_mod` installs `fs`, `run` and `net` as ordinary
//! modules: `vm.new_module(id)` allocates an object and binds it in the heap's
//! module table, and each verb is a plain map entry on that object
//! (`heap.set_value_def(module, method, fn_obj)`).
//!
//! So the strip does not enumerate verb names — that would leave a hole the
//! day someone adds one. It **rebinds the whole module to a fresh empty
//! object**, which drops every method the module had, present and future. The
//! original object is left unreferenced for the GC.
//!
//! This runs immediately after the isolate is allocated and before any game
//! source is evaluated, so no script has had the chance to capture an alias to
//! the old module.

use makepad_widgets::*;

/// Whether a game's isolate may reach the host machine.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Trust {
    /// Authored locally by the person at the keyboard.
    #[default]
    Local,
    /// Arrived from a registry or a peer. Capability-stripped.
    Downloaded,
}

impl Trust {
    pub fn is_sandboxed(self) -> bool {
        self == Trust::Downloaded
    }
}

/// Modules a downloaded game has no business reaching.
///
/// - `fs`: reads and writes anywhere the process can.
/// - `run`: spawns child processes.
/// - `net`: opens sockets and HTTP servers.
///
/// Note what is deliberately *kept*: `std` (assert, ranges, maths), `math`,
/// `pod`, `gc` and the widget/theme modules — a game needs those, and none of
/// them reach outside the VM.
pub const STRIPPED_MODULES: &[LiveId] = &[live_id!(fs), live_id!(run), live_id!(net)];

/// Remove host-reaching capabilities from an isolate. Idempotent.
pub fn strip_capabilities(vm: &mut ScriptVm) {
    for id in STRIPPED_MODULES {
        // Rebinds mod.<id> to a fresh empty module; every verb the std
        // installed on the old object becomes unreachable.
        vm.new_module(*id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Evaluate a snippet in a fresh isolate, optionally stripped, and report
    /// the errors the VM captured.
    fn eval_in_isolate(cx: &mut Cx, trust: Trust, code: &str) -> Vec<String> {
        let vm_id = cx.alloc_splash_vm_with_network(true);
        cx.with_script_vm_id(vm_id, |vm| {
            if trust.is_sandboxed() {
                strip_capabilities(vm);
            }
            vm.bx.captured_errors = Some(Vec::new());
            let script_mod = ScriptMod {
                cargo_manifest_path: String::new(),
                module_path: String::new(),
                file: "sandbox_test.splash".to_string(),
                line: 0,
                column: 9_000_000 + code.len(),
                code: String::new(),
                values: vec![],
            };
            let _ = vm.eval_with_append_source(script_mod, &format!("{code}\n;"), NIL.into());
            vm.take_errors()
        })
    }

    /// The capabilities that must be gone, each with the call a hostile game
    /// would actually make.
    const ESCAPES: &[(&str, &str)] = &[
        ("read a file", "let x = mod.fs.read(\"/etc/passwd\")"),
        ("write a file", "mod.fs.write(\"/tmp/pwned\", \"x\")"),
        ("read to string", "let x = mod.fs.read_to_string(\"/etc/hosts\")"),
        ("spawn a process", "let c = mod.run.child(\"sh\")"),
    ];

    #[test]
    fn a_downloaded_game_cannot_reach_the_host() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        for (what, code) in ESCAPES {
            let errors = eval_in_isolate(&mut cx, Trust::Downloaded, code);
            assert!(
                !errors.is_empty(),
                "a sandboxed isolate allowed it to {what}: {code:?}"
            );
        }
    }

    #[test]
    fn the_stripped_modules_are_empty_not_merely_shadowed() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        // Enumerating the module must find nothing — this is the check that
        // does not depend on knowing every verb's name.
        let vm_id = cx.alloc_splash_vm_with_network(true);
        cx.with_script_vm_id(vm_id, |vm| {
            strip_capabilities(vm);
            for id in STRIPPED_MODULES {
                let module = vm.module(*id);
                let count = vm.map_mut_with(module, |_vm, map| map.iter().count());
                assert_eq!(count, 0, "module {id:?} still has {count} entries");
            }
        });
    }

    #[test]
    fn a_local_game_keeps_its_capabilities() {
        // The counterweight: if stripping were a no-op in reverse — that is, if
        // these calls failed for unrelated reasons — the test above would pass
        // vacuously. An unstripped isolate must genuinely reach fs.
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let path = std::env::temp_dir().join(format!("makepad-sandbox-{}.txt", std::process::id()));
        let _ = std::fs::write(&path, b"visible");
        let code = format!("let x = mod.fs.read_to_string({:?})", path.to_string_lossy());
        let errors = eval_in_isolate(&mut cx, Trust::Local, &code);
        assert!(
            errors.is_empty(),
            "an unstripped isolate should still read files, got {errors:?}"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn stripping_is_idempotent() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let vm_id = cx.alloc_splash_vm_with_network(true);
        cx.with_script_vm_id(vm_id, |vm| {
            strip_capabilities(vm);
            strip_capabilities(vm);
            let module = vm.module(live_id!(fs));
            let count = vm.map_mut_with(module, |_vm, map| map.iter().count());
            assert_eq!(count, 0);
        });
    }
}
