//! Field array lengths whose constants need executed CTFE (`Type::CONST` is not folded), first
//! normalized while lowering ahead of a build: a padded type's layout in a generic instance
//! (`size_of::<H>()`), its callers holding only a Box or a reference. The types live in this
//! dependency, so `makepad check --root-only` reaches them through callees lowered ahead, as
//! makepad-draw's `_pad: [f32; DrawX::__RX_DRAW_PADDING]` fields. Lowering ahead may not take
//! the unit lock and unwinds; the constant evaluation that unwind crosses must be cancelled,
//! not cached as the constant's failure. Cached, it gave E0080, `[f32; {error}]` lengths and a
//! field table static that failed to type-check (then read through a null address).
const fn pad(n: usize) -> usize {
    let mut size = 0usize;
    let mut i = 0;
    while i < n {
        size += 4;
        i += 1;
    }
    (16 - size % 16) % 16 / 4
}

pub struct Field {
    pub name: &'static str,
    pub offset: usize,
}

pub trait Fields {
    fn fields() -> &'static [Field];
}

#[inline(never)]
fn lanes<H>(_: &H) -> usize {
    core::mem::size_of::<H>() / 4
}

#[inline(never)]
fn size_of_val_generic<H>(h: &H) -> usize {
    assert_eq!(core::mem::size_of_val(h), core::mem::size_of::<H>());
    core::mem::size_of_val(h)
}

#[inline(never)]
fn fields_of<H: Fields>(_: &H) -> &'static [Field] {
    H::fields()
}

macro_rules! padded {
    ($($s:ident $check:ident $k:literal $p:literal),* $(,)?) => {
        $(
            pub struct $s {
                pub v: [f32; $k],
                pub _pad: [f32; $s::PAD],
            }
            impl $s {
                pub const PAD: usize = pad($k);
                pub const FIELDS: &'static [Field] = {
                    static FIELDS: [Field; 2] = [
                        Field { name: "v", offset: core::mem::offset_of!($s, v) },
                        Field { name: "_pad", offset: core::mem::offset_of!($s, _pad) },
                    ];
                    &FIELDS
                };
                // No field is named: type checking leaves the padding length unevaluated.
                #[inline(never)]
                pub fn boxed() -> Box<Self> {
                    Box::new(unsafe { core::mem::zeroed() })
                }
            }
            impl Fields for $s {
                fn fields() -> &'static [Field] {
                    Self::FIELDS
                }
            }
            // A live session (`makepad run`) gives live-editable structs a hidden extension word
            // after their fields: sizes are compared with each other, not with rustc's. This
            // body never needs the padded type's layout itself (only its generic callees do).
            #[inline(never)]
            fn $check() -> usize {
                let s = $s::boxed();
                let n = lanes(&*s);
                assert_eq!(n * 4, size_of_val_generic(&*s));
                assert!(n >= $k + $p);
                let fields = fields_of(&*s);
                assert_eq!(fields[1].name, "_pad");
                assert_eq!(fields[1].offset, 4 * $k);
                $k + $p
            }
        )*
        /// The padded types' field lanes, summed (each type checked against its layout).
        pub fn run() -> usize {
            let mut total = 0;
            $(total += $check();)*
            total
        }
    };
}

// name, check fn, field lanes, padding lanes (to a 16-byte multiple)
padded! {
    S0 c0 1 3, S1 c1 2 2, S2 c2 3 1, S3 c3 5 3, S4 c4 6 2, S5 c5 7 1, S6 c6 9 3, S7 c7 10 2,
    S8 c8 1 3, S9 c9 2 2, S10 c10 3 1, S11 c11 5 3, S12 c12 6 2, S13 c13 7 1, S14 c14 9 3, S15 c15 10 2,
    S16 c16 1 3, S17 c17 2 2, S18 c18 3 1, S19 c19 5 3, S20 c20 6 2, S21 c21 7 1, S22 c22 9 3, S23 c23 10 2,
    S24 c24 1 3, S25 c25 2 2, S26 c26 3 1, S27 c27 5 3, S28 c28 6 2, S29 c29 7 1, S30 c30 9 3, S31 c31 10 2,
    S32 c32 1 3, S33 c33 2 2, S34 c34 3 1, S35 c35 5 3, S36 c36 6 2, S37 c37 7 1, S38 c38 9 3, S39 c39 10 2,
    S40 c40 1 3, S41 c41 2 2, S42 c42 3 1, S43 c43 5 3, S44 c44 6 2, S45 c45 7 1, S46 c46 9 3, S47 c47 10 2,
}
