//! Stacks as an agent reads them: inlined copies expanded into their logical frames (the inline
//! stack), the program's frames in full, Relax std and runtime frames folded, what the top frame
//! means (sleeping, a futex wait, the event loop) in words.

use super::{s, short_fn, Names};
use makepad_strict_json::Value;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// the program's own code (JIT, not std)
    Program,
    /// Relax's std / core (JIT)
    Std,
    /// host code: makepad run, system libraries
    Runtime,
}

#[derive(Clone, Debug)]
pub struct LFrame {
    pub name: String,
    pub at: String,
    pub kind: Kind,
    /// "O3", "baseline (replaced)", "O1 warm-cache"; empty for host frames
    pub tier: String,
    /// the slot of the physical code (drill-down handle S<n>), for the physical level only
    pub slot: Option<i64>,
    /// a level of an inlined copy (not the physical fn)
    pub inlined: bool,
    /// the host image of a runtime frame
    pub module: String,
}

pub struct Frames {
    pub f: Vec<LFrame>,
}

/// What a top host frame means.
pub fn meaning(top: &str) -> Option<&'static str> {
    Some(match top {
        "__semwait_signal" | "nanosleep" | "clock_nanosleep" => "sleeping",
        "__ulock_wait" | "__ulock_wait2" | "futex_wait" | "syscall" => "futex wait",
        "__psynch_cvwait" | "pthread_cond_wait" => "condvar wait",
        "__psynch_mutexwait" => "mutex wait",
        "mach_msg2_trap" | "mach_msg_trap" => "mach message wait (event loop)",
        "__workq_kernreturn" | "start_wqthread" => "idle dispatch worker",
        "semaphore_wait_trap" | "semaphore_timedwait_trap" => "semaphore wait",
        "kevent" | "kevent64" | "kevent_id" | "epoll_wait" | "poll" | "__select" => "event wait",
        "read" | "__read_nocancel" | "recvfrom" | "__recvfrom" | "accept" | "__accept" => "blocking I/O",
        _ => return None,
    })
}

impl Frames {
    pub fn from(list: &[Value]) -> Frames {
        let mut f = Vec::new();
        for fr in list {
            let name = s(fr, "fn").to_string();
            let module = s(fr, "in").to_string();
            let code = fr.get("code");
            if !module.starts_with("jit") || name.starts_with("jit 0x") {
                f.push(LFrame { name, at: String::new(), kind: Kind::Runtime, tier: String::new(), slot: None, inlined: false, module });
                continue;
            }
            let tier = match code {
                Some(c) => {
                    let mut t = s(c, "tier").to_string();
                    let o = s(c, "origin");
                    if !o.is_empty() && o != "fresh" {
                        t.push(' ');
                        t.push_str(o);
                    }
                    let st = s(c, "state");
                    if !st.is_empty() && st != "installed" {
                        t.push_str(&format!(" ({})", st));
                    }
                    t
                }
                None => String::new(),
            };
            let slot = code.and_then(|c| super::num(c.get("slot"))).map(|x| x as i64);
            let levels = code.map(|c| super::arr(c, "inline_stack")).unwrap_or(&[]);
            if levels.len() >= 2 {
                for (k, l) in levels.iter().enumerate() {
                    let last = k + 1 == levels.len();
                    let n = s(l, "fn").to_string();
                    let at = s(l, "at").to_string();
                    f.push(LFrame { kind: kind_of(&at, &n), name: n, at, tier: tier.clone(), slot: if last { slot } else { None }, inlined: !last, module: module.clone() });
                }
                continue;
            }
            let at = s(fr, "at").to_string();
            // an inlined copy without a recorded inline stack: the fn it is inlined into is a frame too
            let into = code.map(|c| s(c, "inlined_into").to_string()).unwrap_or_default();
            if !into.is_empty() {
                f.push(LFrame { kind: kind_of(&at, &name), name, at: at.clone(), tier: tier.clone(), slot: None, inlined: true, module: module.clone() });
                f.push(LFrame { kind: kind_of("", &into), name: into, at: String::new(), tier, slot, inlined: false, module });
                continue;
            }
            f.push(LFrame { kind: kind_of(&at, &name), name, at, tier, slot, inlined: false, module });
        }
        Frames { f }
    }

    pub fn first_program(&self) -> Option<&LFrame> {
        self.f.iter().find(|x| x.kind == Kind::Program && !x.name.starts_with("(inlined"))
    }

    /// What the thread is doing, in a few words: "sleeping in time::sleep, from relax_inspect_eval::main
    /// (inspect_eval.rs:128)".
    pub fn doing(&self, names: &mut Names) -> String {
        let Some(top) = self.f.first() else { return "no stack".into() };
        let what = if top.kind == Kind::Runtime { meaning(&top.name).map(str::to_string) } else { None };
        let first_jit = self.f.iter().find(|x| x.kind != Kind::Runtime);
        let prog = self.first_program();
        let mut t = match (&what, first_jit) {
            (Some(w), Some(j)) => format!("{} in {}", w, short_fn(&j.name)),
            (Some(w), None) => format!("{} ({})", w, top.name),
            (None, _) => format!("in {}", short_fn(&top.name)),
        };
        if let Some(p) = prog {
            if first_jit.is_none_or(|j| j.name != p.name) || what.is_none() && top.name != p.name {
                t.push_str(&format!(", from {}", short_fn(&p.name)));
            }
            if !p.at.is_empty() {
                t.push_str(&format!(" ({})", names.at(&p.at)));
            }
        }
        t
    }

    /// The stack as lines: the top frame, the program's frames, the std frame each program frame
    /// calls, the rest folded; frames below the outermost program frame left out.
    pub fn compact(&self, names: &mut Names, indent: &str) -> String {
        let n = self.f.len();
        if n == 0 {
            return format!("{}(no frames)\n", indent);
        }
        let last_prog = self.f.iter().rposition(|x| x.kind == Kind::Program);
        let end = last_prog.map_or(n.min(6), |i| i + 1);
        let mut show = vec![false; n];
        show[0] = true;
        for i in 0..end {
            if self.f[i].kind == Kind::Program {
                show[i] = true;
                // the callee it went into (the std / runtime frame just above)
                if i > 0 {
                    show[i - 1] = true;
                }
            }
        }
        if last_prog.is_none() {
            for x in show.iter_mut().take(end) {
                *x = true;
            }
        }
        let mut out = String::new();
        let mut i = 0;
        while i < end {
            if show[i] {
                out.push_str(indent);
                out.push_str(&self.line(i, names));
                out.push('\n');
                i += 1;
                continue;
            }
            let j = (i..end).find(|&k| show[k]).unwrap_or(end);
            let kinds: Vec<&str> = {
                let mut v: Vec<&str> = self.f[i..j].iter().map(|x| if x.kind == Kind::Std { "std" } else { "runtime" }).collect();
                v.dedup();
                v
            };
            out.push_str(&format!("{}… {} {} frame{}\n", indent, j - i, kinds.join("+"), if j - i == 1 { "" } else { "s" }));
            i = j;
        }
        if end < n {
            out.push_str(&format!("{}(+{} frames below)\n", indent, n - end));
        }
        out
    }

    fn line(&self, i: usize, names: &mut Names) -> String {
        let x = &self.f[i];
        let mut t = short_fn(&x.name);
        if x.kind == Kind::Runtime {
            if let Some(m) = meaning(&x.name) {
                t = format!("{} ({})", m, x.name);
            }
            if !x.module.is_empty() && x.module != "makepad" {
                t.push_str(&format!(" [{}]", x.module));
            }
            return t;
        }
        if !x.at.is_empty() {
            t.push_str(&format!(" @ {}", names.at(&x.at)));
        }
        let mut tags = Vec::new();
        if x.inlined {
            tags.push("inlined".to_string());
        } else if !x.tier.is_empty() {
            tags.push(x.tier.clone());
        }
        if let Some(sl) = x.slot {
            if x.kind == Kind::Program {
                tags.push(format!("S{}", sl));
            }
        }
        if !tags.is_empty() {
            t.push_str(&format!(" [{}]", tags.join(" ")));
        }
        t
    }
}

/// By the fn's path first (generated code's line table can point into std), by the file for an
/// unidentified inline level.
pub fn kind_of(at: &str, name: &str) -> Kind {
    let n = name.strip_prefix("{closure} in ").unwrap_or(name).trim_start_matches('<');
    if ["core::", "std::", "alloc::"].iter().any(|p| n.starts_with(p)) {
        return Kind::Std;
    }
    if (name.starts_with("(inlined") || name.is_empty()) && (at.contains("/relax/compiler/std/") || at.starts_with("std/")) {
        return Kind::Std;
    }
    Kind::Program
}
