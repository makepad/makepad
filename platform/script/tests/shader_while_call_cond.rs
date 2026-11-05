//! A `while` whose condition calls a function on a var the body reassigns
//! (render's CSM cascade search: `while ci < 3.5 && self.csm_inside(q, 0.99)
//! < 0.5 { ci = ci + 1.0  q = self.csm_proj(min(ci, 3.0), wp) }`) re-tests
//! the whole condition, call included, at the top of every pass, on every
//! backend. The GPU result is checked in the platform's metal_gpu_tests.
use makepad_script::*;
use makepad_shader::spirv::{OP_BRANCH, OP_BRANCH_CONDITIONAL, OP_FUNCTION_CALL, OP_LABEL, OP_LOAD, OP_LOOP_MERGE, OP_SELECTION_MERGE, OP_STORE};

fn compile(backend: &str, fragment_body: &str, extra: &str) -> String {
    wgsl_plain(&emit(backend, fragment_body, extra))
}

/// The backend's output as the seam returns it (SPIR-V: hex words).
fn emit(backend: &str, fragment_body: &str, extra: &str) -> String {
    let host = Box::leak(Box::new(ScriptVmHost::new(0i32, ())));
    let mut vm = ScriptVm {
        host: host.host_context(),
        bx: Box::new(ScriptVmBase::new()),
    };
    vm.bx.captured_errors = Some(Vec::new());
    let code = format!("use mod.pod.*\nuse mod.math.*\nuse mod.shader\nlet sh = {{\nvertex_pos: shader.vertex_position(vec4f)\npixel: shader.fragment_output(0, vec4f)\nu_n: shader.uniform(0.0)\n{extra}\nvertex: fn() {{\nself.vertex_pos = vec4(0.0, 0.0, 0.0, 1.0)\n}}\nfragment: fn() {{\n{fragment_body}\n}}\n}}\nshader.test_compile_draw_source(sh, \"{backend}\", false)");
    let value = vm.with_instruction_limit(5_000_000, |vm| vm.eval(ScriptMod::new("", "", "x", 0, 0, &code, &[])));
    vm.bx.heap.string_with(value, |_, s| s.to_string()).unwrap_or_else(|| format!("ERRORS: not a string {value:?}"))
}

/// The IR printers' locals (`l3_f`) and WGSL's typed literals (`2.0f`, `0i`)
/// as the other emitters spell them (`l_f`, `2.0`, `0`), so one needle checks
/// every backend.
fn wgsl_plain(src: &str) -> String {
    let b = src.as_bytes();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        let prev_ident = i > 0 && (b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_');
        // `l<digits>_` at the start of an identifier.
        if c == b'l' && !prev_ident {
            let mut j = i + 1;
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            if j > i + 1 && j < b.len() && b[j] == b'_' {
                out.push_str("l_");
                i = j + 1;
                continue;
            }
        }
        // A number's `f` / `i` suffix.
        if (c == b'f' || c == b'i') && i > 0 && b[i - 1].is_ascii_digit() && !(i + 1 < b.len() && (b[i + 1].is_ascii_alphanumeric() || b[i + 1] == b'_')) {
            let mut j = i;
            while j > 0 && (b[j - 1].is_ascii_digit() || b[j - 1] == b'.' || b[j - 1] == b'e' || b[j - 1] == b'-') {
                j -= 1;
            }
            let starts_number = j == 0 || !(b[j - 1].is_ascii_alphanumeric() || b[j - 1] == b'_');
            if starts_number {
                i += 1;
                continue;
            }
        }
        out.push(c as char);
        i += 1;
    }
    out
}


/// The emitted body of the loop that follows q's declaration: from its
/// `while(true){` / `while (true) {` / `loop{` to the matching close.
fn loop_body(src: &str) -> &str {
    let start = match (src.find("l_q = "), src.find("l_q: ")) {
        (Some(a), Some(b)) => a.min(b),
        (a, b) => a.or(b).expect("q declared"),
    };
    let rest = &src[start..];
    let open = rest.find("while(true){").or_else(|| rest.find("while (true) {")).or_else(|| rest.find("loop{")).or_else(|| rest.find("loop {")).expect("loop");
    let body = &rest[open..];
    let mut depth = 0;
    for (i, c) in body.char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return &body[..=i];
                }
            }
            _ => {}
        }
    }
    panic!("unclosed loop in\n{src}")
}

/// The pass's exit test of `lp` (lines trimmed) after the loop guard: its
/// line, and the lines that compute it (the guard's next line to the test).
/// Metal, GLSL and WGSL test the condition in one `if`; HLSL short-circuits
/// the call (FXC evaluates both sides of `&&`) into a `[branch] if` that sets
/// a temporary, then tests that.
fn exit_test<'a>(backend: &str, lp: &str, lines: &'a [&'a str]) -> (usize, &'a [&'a str]) {
    let breaks = |l: &&str| l.starts_with("if") && (l.ends_with("{break;}") || l.ends_with("{ break; }"));
    let guard = lines.iter().position(|l| breaks(l) && l.contains("_mp_loop_guard")).unwrap_or_else(|| panic!("{backend}: no loop guard\n{lp}"));
    let test = guard + 1 + lines[guard + 1..].iter().position(breaks).unwrap_or_else(|| panic!("{backend}: no in-loop condition test\n{lp}"));
    (test, &lines[guard + 1..=test])
}

/// The var `arg` names in `cond` (the exit test's lines): `l_q` itself, or a
/// temporary those lines copy it into (`float3 _t6 = l_q;`).
fn read_var<'a>(cond: &[&'a str], arg: &'a str) -> &'a str {
    cond.iter()
        .find_map(|l| l.strip_suffix(';').and_then(|l| l.split_once(" = ")).filter(|(lhs, _)| lhs.ends_with(&format!(" {arg}"))).map(|(_, rhs)| rhs))
        .unwrap_or(arg)
}

/// Whether the exit test (the last line of `cond`) tests the `inside(` call's
/// result: its names, through the assignments of the lines before it (HLSL's
/// `_t9 = (!_t5)`, `_t5 = _t8`, `_t8 = (_t7 < 0.5)`, `_t7 = io_inside(..)`).
fn tests_the_call(cond: &[&str]) -> bool {
    let words = |s: &str| s.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).filter(|w| !w.is_empty()).map(str::to_string).collect::<Vec<_>>();
    let (exit, before) = cond.split_last().expect("an exit test");
    if exit.contains("inside(") {
        return true;
    }
    let (mut todo, mut seen) = (words(exit), Vec::new());
    while let Some(w) = todo.pop() {
        if seen.contains(&w) {
            continue;
        }
        for (lhs, rhs) in before.iter().filter_map(|l| l.strip_suffix(';').and_then(|l| l.split_once(" = "))) {
            if lhs == w || lhs.ends_with(&format!(" {w}")) {
                if rhs.contains("inside(") {
                    return true;
                }
                todo.extend(words(rhs));
            }
        }
        seen.push(w);
    }
    false
}

/// The fragment's SPIR-V words from the `spirv` seam's `f:` hex line.
fn spirv_fragment(src: &str) -> Vec<u32> {
    let hex = src.lines().find_map(|l| l.strip_prefix("f:")).expect("fragment words");
    (0..hex.len()).step_by(8).map(|i| u32::from_str_radix(&hex[i..i + 8], 16).unwrap()).collect()
}

/// SPIR-V: in the loop, the call that tests the condition (between the guard's
/// break and the exit test's) takes q as loaded in that pass, the exit test's
/// branch condition is computed from that call's result, and the body after
/// the exit test stores q from a call: the var the test reads.
fn spirv_retests(words: &[u32]) {
    let mut inst = Vec::new();
    let mut at = 5;
    while at < words.len() {
        let len = (words[at] >> 16) as usize;
        assert!(len > 0 && at + len <= words.len(), "spirv: bad instruction at word {at}");
        inst.push(&words[at..at + len]);
        at += len;
    }
    let op = |i: &[u32]| i[0] & 0xffff;
    let head = inst.iter().position(|i| op(i) == OP_LOOP_MERGE).expect("spirv: a loop");
    let merge = inst[head][1];
    let end = head + inst[head..].iter().position(|i| op(i) == OP_LABEL && i[1] == merge).expect("spirv: the loop's merge block");
    let lp = &inst[head..end];
    let breaks: Vec<usize> = (0..lp.len()).filter(|&i| op(lp[i]) == OP_BRANCH && lp[i][1] == merge).collect();
    assert_eq!(breaks.len(), 2, "spirv: the guard's break and the exit test's");
    let calls: Vec<&[u32]> = lp[breaks[0]..breaks[1]].iter().copied().filter(|i| op(i) == OP_FUNCTION_CALL).collect();
    assert_eq!(calls.len(), 1, "spirv: one call in the condition");
    // the exit test: the conditional branch into the block that breaks, whose
    // condition flows (through the short-circuit's compare, phi and not) from
    // the call's result
    let exit_block = lp[breaks[1] - 1];
    assert_eq!(op(exit_block), OP_LABEL, "spirv: the exit test's break is its own block");
    let exit = lp[..breaks[1]].iter().rev().find(|i| op(i) == OP_BRANCH_CONDITIONAL && (i[2] == exit_block[1] || i[3] == exit_block[1])).expect("spirv: the exit test's branch");
    let no_result = [OP_STORE, OP_BRANCH, OP_BRANCH_CONDITIONAL, OP_SELECTION_MERGE, OP_LOOP_MERGE, OP_LABEL];
    let defs: Vec<&[u32]> = lp[breaks[0]..breaks[1]].iter().copied().filter(|i| i.len() > 2 && !no_result.contains(&op(i))).collect();
    let (mut todo, mut seen) = (vec![exit[1]], Vec::new());
    let mut from_call = false;
    while let Some(id) = todo.pop() {
        if seen.contains(&id) {
            continue;
        }
        seen.push(id);
        from_call |= id == calls[0][2];
        if let Some(d) = defs.iter().find(|d| d[2] == id) {
            todo.extend_from_slice(&d[3..]);
        }
    }
    assert!(from_call, "spirv: the exit test's condition does not come from the call");
    let q_arg = calls[0][4];
    let q_var = lp[breaks[0]..breaks[1]].iter().find(|i| op(i) == OP_LOAD && i[2] == q_arg).map(|i| i[3]).expect("spirv: the call's q loaded in the pass");
    let body = &lp[breaks[1]..];
    let reassigned = body.iter().any(|st| op(st) == OP_STORE && st[1] == q_var && body.iter().any(|c| op(c) == OP_FUNCTION_CALL && c[2] == st[2]));
    assert!(reassigned, "spirv: the body after the exit test stores the var the condition read from a call");
}

#[test]
fn a_while_with_a_called_condition_retests_it_every_pass() {
    let extra = "inside: fn(q: vec3, m: float) -> float {\nif q.x > m { return 0.0 }\nreturn 1.0\n}\nproj: fn(ci: float) -> vec3 {\nreturn vec3(2.0 - ci + self.u_n, 0.0, 0.0)\n}";
    let body = "var ci = 0.0\nvar q = self.proj(0.0)\nwhile ci < 3.5 && self.inside(q, 0.99) < 0.5 {\nci = ci + 1.0\nq = self.proj(min(ci, 3.0))\n}\nself.pixel = vec4(ci, q.x, 0.0, 1.0)";
    for backend in ["metal", "hlsl", "glsl", "wgsl"] {
        let src = compile(backend, body, extra);
        assert!(!src.starts_with("ERRORS"), "{backend}: {src}");
        let lp = loop_body(&src);
        let lines: Vec<&str> = lp.lines().map(str::trim).collect();
        // The guard, then the whole condition (the call included) as the
        // pass's exit test, then the body's two reassignments, in order.
        let (exit, cond) = exit_test(backend, lp, &lines);
        assert!(cond.iter().any(|l| l.contains("< 3.5")) && cond.iter().any(|l| l.contains("inside(")), "{backend}: the exit test is not the whole condition\n{lp}");
        assert!(tests_the_call(cond), "{backend}: the exit test does not test the call's result\n{lp}");
        let ci = lines.iter().position(|l| l.starts_with("l_ci = (l_ci + 1")).unwrap_or_else(|| panic!("{backend}: no ci step\n{lp}"));
        let q = lines.iter().position(|l| l.starts_with("l_q = ") && l.contains("proj(") && l.contains("min(l_ci, 3.0)")).unwrap_or_else(|| panic!("{backend}: no q reassignment\n{lp}"));
        assert!(exit < ci && ci < q, "{backend}: condition, step, reassignment out of order\n{lp}");
        // The reassigned var is the one the condition reads: no copy made
        // before the loop stands in for it.
        let call = cond.iter().find(|l| l.contains("inside(")).unwrap();
        let arg = call.split("inside(").nth(1).and_then(|a| a.split(", 0.99").next()).and_then(|a| a.rsplit(", ").next()).unwrap_or_else(|| panic!("{backend}: {call}"));
        assert_eq!(read_var(cond, arg), "l_q", "{backend}: the condition's call reads {arg}\n{lp}");
        // Declared once, before the loop (a declaration inside would shadow).
        assert!(!lp.contains("float3 l_q") && !lp.contains("vec3 l_q") && !lp.contains("var l_q"), "{backend}: q redeclared in the loop\n{lp}");
    }
    let src = emit("spirv", body, extra);
    assert!(!src.starts_with("ERRORS"), "spirv: {src}");
    spirv_retests(&spirv_fragment(&src));
}
