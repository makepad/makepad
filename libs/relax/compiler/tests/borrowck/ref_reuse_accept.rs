// rustc accepts (Scope code_view prepared_lod.rs, agent_activity compact_history, bytemuck
// checked.rs): a reference to a projection (`&S::File`, `&<T as Tr>::Bits`) is Copy -- in a
// polymorphic body the projection is rigid and `&'a T: Copy` binds T to it (the solver related
// the variable and the rigid alias as a new alias-relate goal, again and again, until overflow:
// ambiguous, so not Copy) -- and a `&mut T` coerced to `&T` inside `Some(..)` on the right of
// `==` is reborrowed, not moved. Generic bodies are checked polymorphically
// (MAKEPAD_RUN_BORROWCK_ALL=1 in run_live_regressions.sh), as Gandalf does while a program runs.
use std::collections::HashMap;
use std::path::PathBuf;

trait LodFile {
    fn bytes(&self) -> u64;
}

trait LodSource {
    type File: LodFile;
    fn seek_page(&mut self, file: &Self::File, line: u64) -> Option<u64>;
}

struct Text(u64);
impl LodFile for Text {
    fn bytes(&self) -> u64 {
        self.0
    }
}

struct Src(u32);
impl LodSource for Src {
    type File = Text;
    fn seek_page(&mut self, file: &Text, line: u64) -> Option<u64> {
        self.0 += 1;
        Some(file.0 + line)
    }
}

struct Window<S: LodSource> {
    pin: Option<u64>,
    source: Option<S>,
}

impl<S: LodSource> Window<S> {
    fn seek(&mut self, source: &mut S, file: &S::File, line: u64) -> Result<u64, String> {
        let pin = source.seek_page(file, line).ok_or("no page".to_string())?;
        self.pin = Some(pin);
        if pin > file.bytes() {
            return Err("beyond".to_string());
        }
        Ok(pin)
    }
}

struct Cursor<S: LodSource = Src> {
    window: Window<S>,
}

impl<S: LodSource> Cursor<S> {
    fn rows(&mut self, source: &mut S, file: &S::File, lines: u64) -> Result<u64, String> {
        let mut sum = 0;
        for line in 0..lines {
            sum += self.window.seek(source, file, line)?;
        }
        Ok(sum + file.bytes())
    }
}

struct Lod {
    pages: u32,
}

impl Lod {
    fn seek_row<S: LodSource>(&mut self, source: &mut S, file: &S::File, line: u64) -> Option<u64> {
        let page = source.seek_page(file, line)?;
        self.pages += 1;
        if page > file.bytes() {
            return None;
        }
        Some(page)
    }

    fn each_line<S: LodSource>(&mut self, source: &mut S, file: &S::File, lines: u64) -> u64 {
        let mut sum = 0;
        for line in 0..lines {
            sum += source.seek_page(file, line).unwrap_or(0);
        }
        sum + file.bytes()
    }
}

enum Op {
    Read { path: PathBuf },
    Other,
}

fn compact(history: &mut Vec<(u32, Op)>) -> usize {
    let mut last_read: HashMap<u32, PathBuf> = HashMap::new();
    let mut kept = 0;
    for (session, mut op) in history.drain(..) {
        match &mut op {
            Op::Read { path } => {
                if last_read.get(&session) == Some(path) {
                    continue;
                }
                last_read.insert(session, path.clone());
            }
            Op::Other => {}
        }
        kept += 1;
    }
    kept
}

fn a01_assoc_ref_is_copy() {
    let mut lod = Lod { pages: 0 };
    let mut src = Src(0);
    let file = Text(10);
    println!("| {:?} {:?}", lod.seek_row(&mut src, &file, 3), lod.seek_row(&mut src, &file, 30));
    println!("| {} {} {}", lod.each_line(&mut src, &file, 4), lod.pages, src.0);
}

fn a01b_impl_param_assoc_ref() {
    let mut c: Cursor = Cursor { window: Window { pin: None, source: None } };
    let mut src = Src(0);
    let file = Text(100);
    for _ in 0..3 {
        println!("| {:?} {:?}", c.rows(&mut src, &file, 3), c.window.pin);
    }
    let mut c2: Cursor<Src2> = Cursor { window: Window { pin: None, source: None } };
    let mut s2 = Src2;
    println!("| {:?} {}", c2.rows(&mut s2, &Text(1), 2), c2.window.source.is_none());
}

struct Src2;
impl LodSource for Src2 {
    type File = Text;
    fn seek_page(&mut self, file: &Text, line: u64) -> Option<u64> {
        Some(file.0 * line)
    }
}

fn a02_mut_ref_reborrowed_in_some() {
    let mut h = vec![
        (1, Op::Read { path: PathBuf::from("a") }),
        (1, Op::Read { path: PathBuf::from("a") }),
        (2, Op::Other),
        (1, Op::Read { path: PathBuf::from("b") }),
    ];
    println!("| kept {}", compact(&mut h));
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
struct SessionId(std::sync::Arc<str>);

#[derive(Clone, Debug, PartialEq)]
enum Operation {
    Read { path: PathBuf, lines: Option<(u32, u32)> },
    Message { summary: String },
    Prompt { summary: String },
    Shell { command: String, code: i32 },
    Other,
}

struct AgentEvent {
    session: SessionId,
    op: Operation,
}

fn shorten(text: &mut String, max: usize) {
    if let Some((cut, _)) = text.char_indices().nth(max) {
        text.truncate(cut);
    }
}

fn compact_history(history: &mut Vec<AgentEvent>) {
    let before = history.len();
    let split = before / 2;
    let mut last_read: HashMap<SessionId, PathBuf> = HashMap::new();
    let mut kept = Vec::with_capacity(before);
    for (index, mut event) in history.drain(..).enumerate() {
        if index < split {
            match &mut event.op {
                Operation::Read { path, .. } => {
                    if last_read.get(&event.session) == Some(path) {
                        continue;
                    }
                    last_read.insert(event.session.clone(), path.clone());
                }
                Operation::Message { summary } | Operation::Prompt { summary } => {
                    shorten(summary, 2);
                    last_read.remove(&event.session);
                }
                Operation::Shell { command, .. } => {
                    shorten(command, 3);
                    last_read.remove(&event.session);
                }
                _ => {
                    last_read.remove(&event.session);
                }
            }
        }
        kept.push(event);
    }
    *history = kept;
}

fn a03_history() {
    let s = |n: &str| SessionId(std::sync::Arc::from(n));
    let read = |p: &str| Operation::Read { path: PathBuf::from(p), lines: None };
    let mut h: Vec<AgentEvent> = vec![
        AgentEvent { session: s("a"), op: read("x") },
        AgentEvent { session: s("a"), op: read("x") },
        AgentEvent { session: s("a"), op: Operation::Message { summary: "hello".into() } },
        AgentEvent { session: s("b"), op: Operation::Shell { command: "ls -la".into(), code: 0 } },
        AgentEvent { session: s("a"), op: Operation::Other },
        AgentEvent { session: s("a"), op: Operation::Prompt { summary: "p".into() } },
        AgentEvent { session: s("a"), op: read("y") },
        AgentEvent { session: s("a"), op: read("y") },
        AgentEvent { session: s("a"), op: read("y") },
        AgentEvent { session: s("a"), op: read("y") },
        AgentEvent { session: s("a"), op: read("y") },
        AgentEvent { session: s("a"), op: read("y") },
        AgentEvent { session: s("a"), op: read("y") },
        AgentEvent { session: s("a"), op: read("y") },
    ];
    compact_history(&mut h);
    println!("| {} {:?}", h.len(), h.iter().map(|e| &e.op).collect::<Vec<_>>()[..3].to_vec());
}

fn take<T: ?Sized>(_: &T) {}

trait Checked {
    type Bits: Copy;
    fn valid(bits: &Self::Bits) -> bool;
}

impl Checked for bool {
    type Bits = u8;
    fn valid(bits: &u8) -> bool {
        *bits < 2
    }
}

fn try_from_bits<T: Checked>(bits: &T::Bits) -> Option<*const T::Bits> {
    if <T as Checked>::valid(bits) {
        Some(bits as *const T::Bits)
    } else {
        None
    }
}

fn copies<S: LodSource>(file: &S::File) -> u64 {
    take(file);
    let a = file;
    let b = file;
    a.bytes() + b.bytes()
}

fn main() {
    println!("| {} {}", try_from_bits::<bool>(&1).is_some(), try_from_bits::<bool>(&7).is_some());
    println!("| {}", copies::<Src>(&Text(4)));
    a03_history();
    a01_assoc_ref_is_copy();
    a01b_impl_param_assoc_ref();
    a02_mut_ref_reborrowed_in_some();
}
