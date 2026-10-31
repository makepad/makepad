// Inference and selection cases from the runner apps' compile check (2026-10-04); each = rustc.
// Lines "| " are compared with rustc.
use std::collections::BTreeSet;
use std::io::Write;

// a bound reached by one dereference before one needing an autoref: BTreeSet<&[String]>
// (search_key's `key.cmp(k.borrow())` with Q = [String], K = &[String])
#[test]
fn i01_bound_method_by_value_first() {
    let a = vec!["x".to_string(), "y".to_string()];
    let b = vec!["z".to_string()];
    let mut s: BTreeSet<&[String]> = BTreeSet::new();
    s.insert(&a[..]);
    s.insert(&b[..]);
    let q: &[String] = &a;
    println!("| {} {}", s.contains(q), s.contains(&["w".to_string()][..]));
}

// `&mut Vec<?T>` as `&mut dyn Write`: the single impl fixes ?T = u8
fn fill(w: &mut dyn Write) -> std::io::Result<()> {
    w.write_all(b"hello")?;
    write!(w, " {}", 42)
}
#[test]
fn i02_unsize_fixes_source() {
    let mut body = Vec::new();
    fill(&mut body).unwrap();
    println!("| {} {}", body.len(), String::from_utf8(body).unwrap());
}

// supertrait methods in a trait's default body win over the Self type's inherent ones
trait Node {
    fn visible(&self) -> bool {
        true
    }
}
trait Widget: Node {
    fn measure(&self, over: Option<bool>) -> f64 {
        if !over.unwrap_or_else(|| self.visible()) {
            return 0.0;
        }
        if self.visible() { 1.0 } else { 2.0 }
    }
}
struct Body;
impl Body {
    fn visible(&self, y: f64) -> bool {
        y > 0.0
    }
}
impl Node for Body {}
impl Widget for Body {}
#[test]
fn i03_supertrait_self_methods() {
    let b = Body;
    println!("| {} {} {}", b.measure(None), b.measure(Some(false)), b.visible(1.0));
}

// a repeat operand coerces to the expected element type
#[test]
fn i04_repeat_coercion() {
    let mut c: [&[i16]; 4] = [&[]; 4];
    let v = vec![1i16, 2, 3];
    c[1] = &v;
    let f: [&dyn std::fmt::Debug; 2] = [&1u8; 2];
    println!("| {:?} {:?}", c, f);
}

// `&str` ordering goes through str's PartialOrd
#[test]
fn i05_str_ordering() {
    let v = vec!["2024-01-01T10:00".to_string(), "2024-01-01T11:00".to_string(), "2024-01-01T12:00".to_string()];
    let now: &str = "2024-01-01T11:30";
    let mut idx = 0;
    for (i, s) in v.iter().enumerate() {
        if s.as_str() <= now {
            idx = i;
        }
    }
    let (a, b) = ("apple", "banana");
    println!("| {} {} {} {} {}", idx, a < b, a >= b, &a[1..] > b, "x" <= "x");
}

// `?` in a closure without a return type: the error type is what the body makes it, the
// earlier `?`s convert through From
fn enc(x: u32) -> Result<(), String> {
    if x > 5 { Err(format!("big {}", x)) } else { Ok(()) }
}
#[test]
fn i06_try_in_open_closure() {
    let p: Option<&str> = Some("a");
    let r = (|| {
        let parent = p.ok_or("no parent")?;
        let _ = parent;
        enc(7)
    })();
    let q: Option<&str> = None;
    let r2 = (|| {
        let v = q.ok_or("none")?;
        Ok(v.len())
    })();
    let r2: Result<usize, &str> = r2;
    println!("| {:?} {:?}", r, r2);
}

// the closure behind `&mut` for `f: &mut impl FnMut(..)` gets the bound's parameter types
struct T {
    nodes: Vec<Option<(u8, f64)>>,
}
impl T {
    fn walk(&self, idx: usize, depth: usize, f: &mut impl FnMut(usize, usize, f64)) {
        f(idx, depth, 1.5);
        if idx + 1 < self.nodes.len() {
            self.walk(idx + 1, depth + 1, f);
        }
    }
    fn best(&self) -> Option<usize> {
        let mut best = None;
        self.walk(0, 0, &mut |idx, depth, r| {
            let Some((a, ratio)) = self.nodes[idx].as_ref() else {
                return;
            };
            if *a > 1 && *ratio + r > 2.0 {
                best = Some(idx * 10 + depth);
            }
        });
        best
    }
}
#[test]
fn i07_closure_through_mut_ref() {
    let t = T { nodes: vec![Some((1, 0.2)), None, Some((3, 0.9))] };
    println!("| {:?}", t.best());
}

// an array of tuples with literal elements: its IntoIterator projection with the literals open
struct S {
    id: usize,
    amount: i32,
    name: String,
}
#[test]
fn i08_array_projection_with_literals() {
    let items = [(1u32, "a", -92_400, 7u8), (2, "b", 5, 8)];
    let v: Vec<S> = items.into_iter().enumerate().map(|(i, (_a, p, amount, _c))| S { id: i + 1, amount, name: p.to_string() }).collect();
    for s in &v {
        println!("| {} {} {}", s.id, s.amount, s.name);
    }
}
