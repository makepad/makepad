// T12 (conform): pre-existing borrowck false positive, rustc accepts; found by N01 Gandalf audit
// (round 52 OB, arch-n01-oB.REPORT.md). Known failure: not listed in run_live_regressions.sh;
// `makepad check` with MAKEPAD_RUN_BORROWCK=full (that script's settings) reports the errors named below,
// as does the pre-N01 tip (which does not check derived impl bodies, so it misses those). Do not
// list it until conform fixes the cause.
// Cause: a struct's inferred outlives predicates (rustc inferred_outlives_of: `K: 'a` from a
// `&'a mut Table<(K, V)>` field, `'a: 'b` from `&'b Line<'a>`) are missing from the implied
// bounds of bodies whose signature or impl self type mentions the struct. Relax reports
// E0309 "K may not live long enough" (std hash_map.rs/btree_map.rs entries) and "'a must outlive
// 'b" (derived Clone's AssertParamIsClone<&'b Line<'a>>, ttf-parser colr.rs GradientStopsIter).
pub struct Table<T>(Vec<T>);
impl<T> Table<T> { pub fn get_mut(&mut self, i: usize) -> &mut T { &mut self.0[i] } }
pub struct Occupied<'a, K, V> { table: &'a mut Table<(K, V)>, index: usize }
impl<'a, K, V> Occupied<'a, K, V> {
    pub fn into_mut(self) -> &'a mut V {
        let table: &'a mut Table<(K, V)> = self.table;
        &mut table.get_mut(self.index).1
    }
}
pub struct Line<'a>(&'a [u8]);
#[derive(Clone, Copy)]
pub struct Stops<'a, 'b> { line: &'b Line<'a>, palette: u16 }
pub struct Stops2<'a, 'b> { line: &'b Line<'a> }
impl<'a, 'b> Stops2<'a, 'b> { fn annotated(&self) { let _: &'b Line<'a>; } }

fn run() {
    let mut table = Table(vec![(1u8, 2u8)]);
    *Occupied { table: &mut table, index: 0 }.into_mut() = 3;
    assert_eq!(table.0[0].1, 3);
    let line = Line(&[1]);
    let stops = Stops { line: &line, palette: 4 };
    let copy = stops.clone();
    assert_eq!(copy.palette + copy.line.0[0] as u16, 5);
    Stops2 { line: &line }.annotated();
}
#[test]
#[ignore = "T12 conform: struct inferred-outlives implied bounds"]
fn adt_implied_outlives() { run() }
fn main() { run() }
