//! Raw identifiers name fields without `r#` (rustc: `r#in` is the identifier `in`): struct definitions,
//! literals, field access, patterns and derived Debug (makepad-motion KeyHandles / asset data UnitSounds).
#[derive(Debug, Default, Clone, PartialEq)]
pub struct KeyHandles { pub r#in: Option<f32>, pub out: Option<f32> }
#[derive(Debug, Default)]
pub struct Sounds { pub r#move: u32, pub r#loop: bool, pub r#type: u8 }
fn get(k: &KeyHandles) -> Option<f32> { k.r#in }
#[test]
fn raw_fields() {
    let mut k = KeyHandles { r#in: Some(1.5), out: None };
    k.r#in = Some(2.0);
    let KeyHandles { r#in, .. } = k.clone();
    assert_eq!((get(&k), r#in), (Some(2.0), Some(2.0)));
    let s = Sounds { r#move: 3, r#loop: true, r#type: 7 };
    assert_eq!((s.r#move, s.r#loop, s.r#type), (3, true, 7));
    assert_eq!(format!("{:?}", k), "KeyHandles { in: Some(2.0), out: None }");
    assert!(k == k.clone());
}
