// Relax's native built-in macros (src/builtins.rs). Expected values come from rustc with the
// real proc macros (makepad-live-id / makepad-script-derive); the differential harness that
// produced them: local/agent_state/fast-rust/tools/builtins/diff.sh.
// run (the `makepad` loader links makepad's id! macros): MAKEPAD_RUN_STD=libs/relax/compiler/std makepad test tools/run/tests/builtins.rs

#[derive(Clone, Copy, PartialEq)]
pub struct LiveId(pub u64);
impl LiveId {
    pub const fn from_num(seed: u64, num: u64) -> Self {
        let b = num.to_be_bytes();
        let mut x = seed;
        let mut i = 0;
        while i < 8 {
            x = x.overflowing_add(b[i] as u64).0;
            x ^= x >> 32;
            x = x.overflowing_mul(0xd6e8_feb8_6659_fd93).0;
            x ^= x >> 32;
            x = x.overflowing_mul(0xd6e8_feb8_6659_fd93).0;
            x ^= x >> 32;
            i += 1;
        }
        Self(x & 0x0000_3fff_ffff_ffff)
    }
}

fn pick(x: LiveId) -> u32 {
    match x {
        id!(apple) => 1,
        id!(banana) => 2,
        _ => 3,
    }
}

mod inner {
    pub fn mp() -> &'static str {
        module_path!()
    }
}

#[test]
fn id_family_matches_rustc() {
    let xs: [LiveId; 14] = [
        id!(apple), id!(), id!(r#type), id!($array), id!(a_b_c), id!(fn), id!(Self), id!(_),
        live_id!(x0), live_id!(draw_bg), some_id!(foo).unwrap(), id!(z9),
        live_id!(very_long_identifier_name_for_hashing), id!(0),
    ];
    let want: [u64; 14] = [
        24341239519098, 0, 30568617325874, 49353809276448, 49434654882476, 55012217286103, 47616252309034,
        20723672253185, 60936781914028, 18320054578807, 21591109008026, 27308275125783, 585744625588, 45978426977841,
    ];
    let mut i = 0;
    while i < 14 {
        assert!(xs[i].0 == want[i]);
        i += 1;
    }
    let a = ids!(a.b.c);
    assert!(a.len() == 3 && a[0].0 == 35342219466370 && a[1].0 == 24818598661546 && a[2].0 == 29102912674847);
    let b = ids!(one.(id!(two)).three);
    assert!(b[0].0 == 4139370007011 && b[1].0 == 10498904301581 && b[2].0 == 5363156162827);
    let arr: &[&[LiveId]] = ids_array!(a.b, c, d.e.f);
    assert!(arr.len() == 3 && arr[0].len() == 2 && arr[1].len() == 1 && arr[2].len() == 3);
    assert!(arr[1][0].0 == 29102912674847 && arr[2][2].0 == 52354895671766);
    assert!(live_id_num!(item, 5).0 == 52312143043854);
    assert!(pick(id!(apple)) == 1 && pick(id!(banana)) == 2 && pick(live_id!(cherry)) == 3);
}

#[test]
fn std_builtins_match_rustc() {
    assert!(stringify!(a + b * (c, d) . e) == "a + b * (c, d).e");
    assert!(stringify!(vm.bx.threads.cur()) == "vm.bx.threads.cur()");
    assert!(concat!("x", 1, "y", 2.5, true, 'c') == "x1y2.5truec");
    assert!(line!() == 72);
    assert!(column!() == 13);
    assert!(inner::mp() == "test_crate::inner");
    assert!(file!().len() > 0);
}

// `f as fn(..)` / `f as usize` on fn items and non-capturing closures (was a lowering ICE,
// reached through objc's add_method(sel!(..), f as extern "C" fn(..)))
pub struct Object {
    x: u32,
}
pub trait Imp {
    fn addr(self) -> usize;
}
impl Imp for extern "C" fn(&Object, u32) {
    fn addr(self) -> usize {
        self as usize
    }
}
extern "C" fn plus1(x: u32) -> u32 {
    x + 1
}

#[test]
fn fn_item_casts() {
    extern "C" fn cb(_this: &Object, _v: u32) {}
    let n = (cb as extern "C" fn(&Object, u32)).addr();
    assert!(n != 0);
    let p = plus1 as extern "C" fn(u32) -> u32;
    assert!(p(41) == 42);
    assert!(plus1 as usize != 0);
    let c = (|x: u32| x * 2) as fn(u32) -> u32;
    assert!(c(21) == 42);
    let r = plus1 as *mut ();
    assert!(r as usize != 0);
}
