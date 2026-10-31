//! Target-independent tests of the assembler core (template pieces, lexing, labels,
//! directives, expressions). Encoder tests live in tests_x64.rs / tests_arm64.rs.
use super::*;

#[test]
fn template_pieces() {
    let names = vec![("x".to_string(), 1u32)];
    let p = parse_template(&["mov {}, {x:e}", "add {0}, {{1}}"], &names, 1, 2, false).unwrap();
    assert_eq!(p, vec![Piece::Op(0, None), Piece::Text(", ".into()), Piece::Op(1, Some('e')), Piece::Text("\nadd ".into()), Piece::Op(0, None), Piece::Text(", {1}".into())].into_iter().skip(0).collect::<Vec<_>>()[..].iter().cloned().map(|x| x).collect::<Vec<_>>().into_iter().fold(vec![Piece::Text("mov ".into())], |mut a, x| { a.push(x); a }));
    assert!(parse_template(&["{2}"], &[], 0, 2, false).is_err());
    assert!(parse_template(&["}"], &[], 0, 0, false).is_err());
}
