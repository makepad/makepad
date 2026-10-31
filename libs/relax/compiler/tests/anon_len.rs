// array lengths / repeat counts are anonymous constants (rustc AnonConst): anything beyond
// literals, paths and arithmetic is evaluated by running it; `Self::C` and `Type::C` resolve;
// an unknown length is an error, never a silent 0
const TABLE: &[u32] = &[1, 2, 3];
struct Joint(u8);
struct Hand { joints: [Joint; Self::COUNT], mask: [u32; TABLE.len() + 1], pad: [u8; (16 - std::mem::size_of::<u64>() % 16) % 16] }
impl Hand { const COUNT: usize = 5; }
#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind { A, B, C }
impl Kind { const COUNT: usize = 3; pub const ALL: [Self; Self::COUNT] = [Kind::A, Kind::B, Kind::C]; }
struct Pool { classes: [Vec<u8>; usize::BITS as usize] }
fn words() -> [u64; std::mem::size_of::<[u8; 32]>() >> 3] { [7; std::mem::size_of::<[u8; 32]>() >> 3] }
#[test]
fn anon_lengths() {
    let h = Hand { joints: [Joint(1), Joint(2), Joint(3), Joint(4), Joint(5)], mask: [0; TABLE.len() + 1], pad: [0; 8] };
    assert_eq!(h.joints.len() + h.mask.len() + h.pad.len(), 5 + 4 + 8);
    assert_eq!(h.joints[4].0, 5);
    assert_eq!(Kind::ALL[2], Kind::C);
    let p = Pool { classes: std::array::from_fn(|_| Vec::new()) };
    assert_eq!(p.classes.len(), 64);
    assert_eq!(words(), [7, 7, 7, 7]);
}
// anonymous array-length consts in impl method signatures belong to the module
#[repr(C)]
pub struct U { pub a: f32, pub b: f32, pub c: f32, pub d: f32 }
impl U {
    pub fn as_slice(&self) -> &[f32; std::mem::size_of::<U>() >> 2] { unsafe { std::mem::transmute(self) } }
}
#[test]
fn impl_sig_len() { let u = U { a: 1.0, b: 2.0, c: 3.0, d: 4.0 }; assert_eq!(u.as_slice()[3], 4.0); }

// DrawShader's tweaker descriptors expand this associated constant through a
// macro. Its anonymous array length must keep the impl's Self binding.
macro_rules! descriptor_order {
    () => {
        const ORDER: [usize; Self::__RX_SHADER_FIELDS.len()] = [1, 0];
    };
}
struct DrawFields;
impl DrawFields {
    const __RX_SHADER_FIELDS: &'static [u32] = &[3, 5];
    descriptor_order!();
    fn fields() -> [u32; Self::__RX_SHADER_FIELDS.len()] { [3, 5] }
}
struct OtherFields;
impl OtherFields {
    const __RX_SHADER_FIELDS: &'static [u32] = &[7, 11, 13];
    const ORDER: [usize; Self::__RX_SHADER_FIELDS.len()] = [2, 1, 0];
}
#[test]
fn self_dependent_descriptor_lengths_keep_their_owner() {
    assert_eq!(DrawFields::ORDER, [1, 0]);
    assert_eq!(DrawFields::fields(), [3, 5]);
    assert_eq!(OtherFields::ORDER, [2, 1, 0]);
}
