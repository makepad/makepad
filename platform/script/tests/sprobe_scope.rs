//! Splash line probes (`sprobe`): a path steps into a vector's lanes, and a probe at a line sees
//! exactly the locals in scope there: a loop body's own lets once they are bound, never a
//! previous pass's leftovers, a finished loop's, or another function's.

use makepad_script::*;

/// Runs `code` (a script file named `sprobe_scope.splash`) with a probe at `line` (the code's
/// 0-based line) and returns the probe's `fields` text of each hit, oldest first.
fn hits(code: &str, line: usize, names: &str) -> Vec<String> {
    let q = |w: &[&str], kv: &[(&str, &str)]| {
        let w: Vec<String> = w.iter().map(|s| s.to_string()).collect();
        let kv: Vec<(String, String)> = kv.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        makepad_script::inspect_query::query(&w, &kv).expect("sprobe is ours")
    };
    let at = format!("sprobe_scope.splash:{line}");
    let added = if names.is_empty() { q(&["sprobe", "add", &at], &[]) } else { q(&["sprobe", "add", &at], &[("code", names)]) };
    let id = added[0].split('"').nth(3).unwrap().to_string();
    let host = Box::leak(Box::new(ScriptVmHost::new(0i32, ())));
    let mut vm = ScriptVm { host: host.host_context(), bx: Box::new(ScriptVmBase::new()) };
    vm.capture_errors();
    vm.with_instruction_limit(500_000, |vm| vm.eval(ScriptMod::new("", "", "sprobe_scope.splash", 0, 0, code, &[])));
    let errs = vm.take_errors();
    assert!(errs.is_empty(), "the script runs: {errs:?}");
    let out = q(&["sprobe", "read", &id], &[("n", "50")]);
    q(&["sprobe", "rm", &id], &[]);
    out.iter().filter(|l| l.starts_with("{\"emit\"")).map(|l| l.split("\"fields\":").nth(1).unwrap().to_string()).collect()
}

fn has(row: &str, name: &str) -> bool {
    row.contains(&format!("\"{name}\":"))
}

#[test]
fn a_probe_reads_vector_lanes_and_sees_the_locals_in_scope_at_its_line() {
    // lines: 0 use, 1 fn other .. 4 }, 5 fn run, 6 hp, 7 total, 8 f (a closure: `run` stays dynamic),
    // 9 for, 10 sq, 11 tag, 12 total +=, 13 }, 14 return, 15 }, 16 run(2)
    let slotted = "use mod.pod.*\nfn other(a) {\n let o = a + 1\n return o\n}\nfn run(n) {\n let hp = vec3(1, 2, 3)\n let total = 0\n let f = || 1\n for i in 0..n {\n  let sq = i * i\n  let tag = sq + 1\n  total += tag\n }\n return total\n}\nrun(2)\n";
    // the same loop in a function the slot compiler takes (no closure), after another such function
    let plain = slotted.replace(" let f = || 1\n", "");

    for (code, base) in [(slotted, 0), (plain.as_str(), -1)] {
        let l = |n: i32| (n + base) as usize;
        // the tag line: a vector's lanes and swizzles, and the loop body's lets
        let rows = hits(code, l(12), "hp.x,hp.z,hp.zy,hp.rgb,hp.w,sq,tag,i");
        assert_eq!(rows.len(), 2, "the loop's two passes hit the line");
        assert!(rows[0].contains("\"hp.x\":\"1\"") && rows[0].contains("\"hp.z\":\"3\""), "a vec3's lanes read: {}", rows[0]);
        assert!(rows[0].contains("\"hp.zy\":\"(3, 2)\"") && rows[0].contains("\"hp.rgb\":\"(1, 2, 3)\""), "swizzles read: {}", rows[0]);
        assert!(rows[0].contains("`w` is not a lane of a vec3f"), "a lane past the vector's size says so: {}", rows[0]);
        assert!(rows[1].contains("\"sq\":\"1\"") && rows[1].contains("\"tag\":\"2\"") && rows[1].contains("\"i\":\"1\""), "the body's own lets, bound, on the second pass: {}", rows[1]);
        // the default list: the same line sees the body's lets; the let's own line does not see
        // them yet (nor the last pass's values); after the loop they are gone; hp shows as a vector
        let rows = hits(code, l(12), "");
        assert!(rows.iter().all(|r| has(r, "sq") && has(r, "tag") && has(r, "hp")), "in scope at the loop body's last line: {rows:?}");
        assert!(rows[0].contains("\"hp\":\"vec3f(1, 2, 3)\""), "a vector shows its lanes: {}", rows[0]);
        let rows = hits(code, l(10), "");
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|r| !has(r, "sq") && !has(r, "tag") && has(r, "i")), "not yet bound on the let's own line, on any pass: {rows:?}");
        let rows = hits(code, l(14), "");
        assert!(rows.iter().all(|r| !has(r, "sq") && !has(r, "tag") && !has(r, "i") && has(r, "total") && has(r, "hp")), "the loop's lets are gone after it: {rows:?}");
        // another function's slots never leak in: `other` ran before, with its own `o`
        assert!(rows.iter().all(|r| !has(r, "o") && !has(r, "a")), "no names of another function: {rows:?}");
    }
}
