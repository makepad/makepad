use makepad_svg::{parse_svg, SvgNode, SvgTextAnchor};

#[test]
fn parses_plain_text() {
    let svg = r#"<svg viewBox="0 0 100 100"><text x="10" y="20" font-size="14">Hello</text></svg>"#;
    let doc = parse_svg(svg);
    let t = match &doc.root[0] {
        SvgNode::Text(t) => t,
        other => panic!("expected Text, got {:?}", other),
    };
    assert_eq!(t.x, 10.0);
    assert_eq!(t.y, 20.0);
    assert_eq!(t.font_size, 14.0);
    assert_eq!(t.content, "Hello");
    assert_eq!(t.text_anchor, SvgTextAnchor::Start);
}

#[test]
fn parses_text_anchor_middle() {
    let svg = r#"<svg><text x="50" y="30" text-anchor="middle">Centered</text></svg>"#;
    let doc = parse_svg(svg);
    let t = match &doc.root[0] {
        SvgNode::Text(t) => t,
        other => panic!("expected Text, got {:?}", other),
    };
    assert_eq!(t.text_anchor, SvgTextAnchor::Middle);
}

#[test]
fn tspans_are_joined_with_newline() {
    // rusty-mermaid emits one <tspan> per visual line (converted from
    // `<br/>`) with `dy` offsets. Our parser injects `\n` between
    // consecutive tspans so the text collector can split + offset lines.
    let svg = r#"<svg><text x="0" y="0"><tspan x="0" dy="1em">Line A</tspan><tspan x="0" dy="1em">Line B</tspan></text></svg>"#;
    let doc = parse_svg(svg);
    let t = match &doc.root[0] {
        SvgNode::Text(t) => t,
        other => panic!("expected Text, got {:?}", other),
    };
    assert_eq!(t.content, "Line A\nLine B");
}

#[test]
fn font_size_defaults_to_16() {
    let svg = r#"<svg><text x="0" y="0">Plain</text></svg>"#;
    let doc = parse_svg(svg);
    let t = match &doc.root[0] {
        SvgNode::Text(t) => t,
        _ => panic!("expected Text"),
    };
    assert_eq!(t.font_size, 16.0);
}
