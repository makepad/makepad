//! A call whose fn declarations carry doc comments, matched after earlier expansions forwarded
//! empty `vis` fragments. The `#` of a desugared doc comment stands where no empty `vis` was
//! forwarded, so `$v:vis` is no option there and the attribute repetition takes it (relax issue 37).
macro_rules! mirrored {
    {
        type Mirror = $two:ident;
        $(
            $(#[$attr:meta])*
            $v:vis fn $fname:ident ($seif:ident : $seifty:ty $(,$argname:ident : $argtype:ty)*) $(-> $ret:ty)? ;
        )*
    } => {
        $(
            $(#[$attr])*
            $v fn $fname($seif: $seifty $(, $argname: $argtype)*) $(-> $ret)? {
                match $seif {
                    $two::Small(s) => s.$fname($($argname),*),
                    $two::Big(b) => b.$fname($($argname),*),
                }
            }
        )*
    };
}

struct Small(u32);
struct Big(u64);

impl Small {
    fn get(&self) -> u64 { self.0 as u64 }
    fn add(&mut self, n: u64) -> u64 { self.0 += n as u32; self.0 as u64 }
    fn scale(&mut self, k: u64) { self.0 *= k as u32; }
}

impl Big {
    fn get(&self) -> u64 { self.0 }
    fn add(&mut self, n: u64) -> u64 { self.0 += n; self.0 }
    fn scale(&mut self, k: u64) { self.0 *= k; }
}

enum Two {
    Small(Small),
    Big(Big),
}

// no `vis`: each expansion forwards an empty one
impl Two {
    mirrored! {
        type Mirror = Two;
        #[inline]
        fn get(self: &Self) -> u64;
        fn scale(self: &mut Self, k: u64);
    }
}

// documented declarations after those
impl Two {
    mirrored! {
        type Mirror = Two;
        /// Adds `n` and gives the new value.
        #[inline]
        #[must_use]
        pub fn add(self: &mut Self, n: u64) -> u64;
    }
}

#[test]
fn documented_calls_after_empty_vis() {
    let mut a = Two::Small(Small(2));
    let mut b = Two::Big(Big(5));
    a.scale(3);
    b.scale(2);
    assert_eq!((a.get(), b.get()), (6, 10), "the undocumented declarations");
    assert_eq!((a.add(1), b.add(4)), (7, 14), "the documented declaration");
}
