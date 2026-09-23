//! Static flowchart.js, Mermaid and Editor.md sequence diagrams. Source stays editable; preview output is
//! generated SVG/pixels, with no scripts or external diagram resources.
use crate::content::{PreviewImage, prepare_image};
use makepad_markdown::escape;
use std::{cell::RefCell, collections::BTreeMap, fmt::Write};

const MAX_SOURCE: usize = 16 * 1024;
thread_local! { static CACHE: RefCell<BTreeMap<String, Result<PreviewImage, String>>> = const { RefCell::new(BTreeMap::new()) }; }

pub fn is_language(language: &str) -> bool {
    matches!(
        language.trim().to_ascii_lowercase().as_str(),
        "flow" | "flowchart" | "mermaid" | "seq" | "sequence"
    )
}

#[derive(Debug)]
struct Node {
    id: String,
    kind: String,
    label: String,
}
#[derive(Debug)]
struct Edge {
    from: usize,
    to: usize,
    label: String,
    direction: String,
}
#[derive(Debug)]
struct Graph {
    nodes: Vec<Node>,
    edges: Vec<Edge>,
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 80
        && id
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
}

fn endpoint(value: &str) -> Result<(&str, String, String), String> {
    let value = value.trim();
    let (id, options) = if let Some((id, options)) = value.split_once('(') {
        (
            id.trim(),
            options
                .strip_suffix(')')
                .ok_or("Unclosed connection options")?,
        )
    } else {
        (value, "")
    };
    if !valid_id(id) {
        return Err(format!("Invalid node identifier: {id}"));
    }
    let mut label = String::new();
    let mut direction = String::new();
    for option in options.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        match option {
            "left" | "right" | "top" | "bottom" => direction = option.into(),
            "yes" | "true" => label = "yes".into(),
            "no" | "false" => label = "no".into(),
            other => {
                if let Some((branch, text)) = other.split_once('@') {
                    if !matches!(branch, "yes" | "no" | "true" | "false") || text.len() > 120 {
                        return Err(format!("Unsupported branch: {other}"));
                    }
                    label = text.into();
                } else {
                    return Err(format!("Unsupported connection option: {other}"));
                }
            }
        }
    }
    Ok((id, label, direction))
}

fn parse(source: &str) -> Result<Graph, String> {
    if source.len() > MAX_SOURCE {
        return Err("Flowchart exceeds 16 KB".into());
    }
    let mut graph = Graph {
        nodes: Vec::new(),
        edges: Vec::new(),
    };
    let mut connections = Vec::new();
    for (index, line) in source.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((id, definition)) = line.split_once("=>") {
            let id = id.trim();
            if !valid_id(id) || graph.nodes.iter().any(|n| n.id == id) {
                return Err(format!(
                    "Line {}: invalid or duplicate node {id}",
                    index + 1
                ));
            }
            let (kind, label) = definition
                .split_once(':')
                .ok_or_else(|| format!("Line {}: expected node type and label", index + 1))?;
            let kind = kind.trim();
            if !matches!(
                kind,
                "start"
                    | "end"
                    | "operation"
                    | "condition"
                    | "inputoutput"
                    | "input"
                    | "output"
                    | "subroutine"
            ) {
                return Err(format!("Unsupported node type: {kind}"));
            }
            // Styling/link suffixes are not interpreted as live HTML or URLs.
            if label.contains(":>") || label.contains('|') {
                return Err("Flow states and node links are not supported".into());
            }
            if label.trim().is_empty() || label.len() > 512 {
                return Err("Node label is empty or too long".into());
            }
            graph.nodes.push(Node {
                id: id.into(),
                kind: kind.into(),
                label: label.trim().replace("\\n", "\n"),
            });
        } else if line.contains("->") {
            connections.push(line);
        } else {
            return Err(format!("Line {}: expected a node or connection", index + 1));
        }
        if graph.nodes.len() > 24 {
            return Err("Flowchart exceeds 24 nodes".into());
        }
    }
    if graph.nodes.is_empty() {
        return Err("Flowchart has no nodes".into());
    }
    for line in connections {
        let parts: Vec<_> = line.split("->").map(endpoint).collect::<Result<_, _>>()?;
        for pair in parts.windows(2) {
            let find = |id: &str| {
                graph
                    .nodes
                    .iter()
                    .position(|n| n.id == id)
                    .ok_or_else(|| format!("Unknown node: {id}"))
            };
            graph.edges.push(Edge {
                from: find(pair[0].0)?,
                to: find(pair[1].0)?,
                label: pair[0].1.clone(),
                direction: pair[0].2.clone(),
            });
            if graph.edges.len() > 48 {
                return Err("Flowchart exceeds 48 connections".into());
            }
        }
    }
    Ok(graph)
}

// Remove only DFS back edges for ranking. They are still drawn as outside
// return paths; joins use the longest forward path, avoiding overlapping nodes.
fn ranks(graph: &Graph) -> Vec<usize> {
    fn visit(
        node: usize,
        graph: &Graph,
        state: &mut [u8],
        order: &mut Vec<usize>,
        back: &mut [bool],
    ) {
        state[node] = 1;
        for (i, edge) in graph
            .edges
            .iter()
            .enumerate()
            .filter(|(_, e)| e.from == node)
        {
            if state[edge.to] == 1 {
                back[i] = true;
            } else if state[edge.to] == 0 {
                visit(edge.to, graph, state, order, back);
            }
        }
        state[node] = 2;
        order.push(node);
    }
    let mut state = vec![0; graph.nodes.len()];
    let mut back = vec![false; graph.edges.len()];
    let mut order = Vec::new();
    for node in 0..graph.nodes.len() {
        if state[node] == 0 {
            visit(node, graph, &mut state, &mut order, &mut back);
        }
    }
    let mut ranks = vec![0; graph.nodes.len()];
    for node in order.into_iter().rev() {
        for (i, edge) in graph
            .edges
            .iter()
            .enumerate()
            .filter(|(_, e)| e.from == node)
        {
            if !back[i] {
                ranks[edge.to] = ranks[edge.to].max(ranks[node] + 1);
            }
        }
    }
    ranks
}

fn lines(label: &str, columns: usize) -> Result<Vec<String>, String> {
    let mut lines = Vec::new();
    for paragraph in label.lines() {
        let mut line = String::new();
        let mut width = 0;
        for ch in paragraph.chars() {
            let advance = if ch.is_ascii() { 1 } else { 2 };
            if width + advance > columns {
                lines.push(line.trim().into());
                line.clear();
                width = 0;
            }
            line.push(ch);
            width += advance;
        }
        lines.push(line.trim().into());
    }
    if lines.len() > 6 {
        return Err("Node label needs more than six lines".into());
    }
    Ok(lines)
}

pub fn svg(language: &str, source: &str) -> Result<String, String> {
    if source.len() > MAX_SOURCE {
        return Err("Diagram exceeds 16 KB".into());
    }
    if !is_language(language) {
        return Err("Unsupported diagram language".into());
    }
    match language.trim().to_ascii_lowercase().as_str() {
        "mermaid" => return crate::mermaid::svg(source),
        "seq" | "sequence" => return crate::mermaid::sequence_svg(source),
        _ => {}
    }
    let graph = parse(source)?;
    let ranks = ranks(&graph);
    let row_count = ranks.iter().max().unwrap() + 1;
    let mut rows = vec![Vec::new(); row_count];
    for (node, rank) in ranks.iter().enumerate() {
        rows[*rank].push(node);
    }
    let columns = rows.iter().map(Vec::len).max().unwrap();
    let width = (columns * 268 + 112) as f64;
    let mut positions = vec![(0.0, 0.0, 0.0, 0.0); graph.nodes.len()];
    let mut labels = Vec::new();
    for node in &graph.nodes {
        labels.push(lines(
            &node.label,
            if node.kind == "condition" { 20 } else { 24 },
        )?);
    }
    let mut y = 24.0;
    for row in &rows {
        let heights: Vec<f64> = row
            .iter()
            .map(|&i| {
                let text = labels[i].len() as f64 * 21.0;
                if graph.nodes[i].kind == "condition" {
                    (text * 2.0 + 24.0).max(108.0)
                } else {
                    (text + 26.0).max(54.0)
                }
            })
            .collect();
        let height = heights.iter().copied().fold(0.0_f64, f64::max);
        for (col, &node) in row.iter().enumerate() {
            let w = match graph.nodes[node].kind.as_str() {
                "start" | "end" => 184.0,
                "condition" => 244.0,
                _ => 224.0,
            };
            positions[node] = (
                width / 2.0 + (col as f64 - (row.len() - 1) as f64 / 2.0) * 268.0,
                y + height / 2.0,
                w,
                heights[col],
            );
        }
        y += height + 62.0;
    }
    let height = y - 38.0;
    if width > 2048.0 || height > 2048.0 {
        return Err("Flowchart exceeds preview dimensions".into());
    }
    let mut svg = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" viewBox="0 0 {width} {height}"><defs><marker id="arrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M 0 0 L 10 5 L 0 10 z" fill="#32634b"/></marker></defs><rect width="100%" height="100%" fill="#ffffff"/><g font-family="Arial, PingFang SC, Noto Sans CJK SC, sans-serif" font-size="16" fill="#193b2b">"##
    );
    let mut returns = 0;
    for edge in &graph.edges {
        let (x1, y1, w1, h1) = positions[edge.from];
        let (x2, y2, w2, h2) = positions[edge.to];
        let (path, lx, ly) = if ranks[edge.to] <= ranks[edge.from] {
            let left = edge.direction == "left";
            let sign = if left { -1.0 } else { 1.0 };
            let lane = if left {
                16.0 + (returns % 3) as f64 * 12.0
            } else {
                width - 16.0 - (returns % 3) as f64 * 12.0
            };
            returns += 1;
            if edge.from == edge.to {
                (
                    format!(
                        "M {},{y1} H {lane} V {} H {x2} V {}",
                        x1 + sign * w1 / 2.0,
                        y2 - h2 / 2.0 - 24.0,
                        y2 - h2 / 2.0 - 2.0
                    ),
                    lane - sign * 14.0,
                    y1 - 10.0,
                )
            } else {
                (
                    format!(
                        "M {},{y1} H {lane} V {y2} H {}",
                        x1 + sign * w1 / 2.0,
                        x2 + sign * (w2 / 2.0 + 2.0)
                    ),
                    lane - sign * 15.0,
                    (y1 + y2) / 2.0,
                )
            }
        } else {
            let start = y1 + h1 / 2.0;
            let end = y2 - h2 / 2.0 - 2.0;
            let mid = (start + end) / 2.0;
            (
                format!("M {x1},{start} V {mid} H {x2} V {end}"),
                if (x1 - x2).abs() < 1.0 {
                    x1 + 23.0
                } else {
                    (x1 + x2) / 2.0
                },
                mid - 7.0,
            )
        };
        write!(svg, r##"<path d="{path}" fill="none" stroke="#32634b" stroke-width="1.8" stroke-linejoin="round" marker-end="url(#arrow)"/>"##).unwrap();
        if !edge.label.is_empty() {
            write!(svg, r##"<text x="{lx}" y="{ly}" text-anchor="middle" font-size="13" stroke="white" stroke-width="5" paint-order="stroke">{}</text>"##, escape(&edge.label)).unwrap();
        }
    }
    for (i, node) in graph.nodes.iter().enumerate() {
        let (x, y, w, h) = positions[i];
        let left = x - w / 2.0;
        let top = y - h / 2.0;
        let shape = match node.kind.as_str() {
            "condition" => format!(
                "<polygon points=\"{x},{top} {},{y} {x},{} {left},{y}\"/>",
                x + w / 2.0,
                y + h / 2.0
            ),
            "inputoutput" | "input" | "output" => format!(
                "<polygon points=\"{},{top} {},{top} {},{} {left},{}\"/>",
                left + 20.0,
                left + w,
                left + w - 20.0,
                top + h,
                top + h
            ),
            _ => format!(
                "<rect x=\"{left}\" y=\"{top}\" width=\"{w}\" height=\"{h}\" rx=\"{}\"/>",
                if matches!(node.kind.as_str(), "start" | "end") {
                    h / 2.0
                } else {
                    4.0
                }
            ),
        };
        write!(
            svg,
            r##"<g fill="#f2f8f4" stroke="#32634b" stroke-width="1.8">{shape}"##
        )
        .unwrap();
        if node.kind == "subroutine" {
            write!(
                svg,
                "<path d=\"M {},{top} v {h} M {},{top} v {h}\"/>",
                left + 10.0,
                left + w - 10.0
            )
            .unwrap();
        }
        svg.push_str("</g>");
        for (line, text) in labels[i].iter().enumerate() {
            let ty = y + (line as f64 - (labels[i].len() - 1) as f64 / 2.0) * 21.0 + 5.5;
            write!(
                svg,
                r#"<text x="{x}" y="{ty}" text-anchor="middle">{}</text>"#,
                escape(text)
            )
            .unwrap();
        }
    }
    svg.push_str("</g></svg>");
    Ok(svg)
}

pub fn render(language: &str, source: &str) -> Result<PreviewImage, String> {
    if source.len() > MAX_SOURCE {
        return Err("Diagram exceeds 16 KB".into());
    }
    let key = blake3::hash(format!("{language}\0{source}").as_bytes())
        .to_hex()
        .to_string();
    if let Some(image) = CACHE.with(|cache| cache.borrow().get(&key).cloned()) {
        return image;
    }
    let image = std::panic::catch_unwind(|| {
        svg(language, source).and_then(|svg| prepare_image(svg.as_bytes()))
    })
    .unwrap_or_else(|_| Err("Diagram renderer could not process this source".into()));
    CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if cache.len() >= 32
            || cache
                .values()
                .filter_map(|v| v.as_ref().ok())
                .map(|v| v.png.len())
                .sum::<usize>()
                + image.as_ref().map_or(0, |v| v.png.len())
                > 16 * 1024 * 1024
        {
            cache.clear();
        }
        cache.insert(key, image.clone());
    });
    image
}

pub fn fallback(language: &str, source: &str, error: &str) -> String {
    format!(
        "<p>Diagram: {}</p>{}",
        escape(error),
        crate::content::code_html(language, source)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    const FLOW: &str = "st=>start: 用户登陆\nop=>operation: 登陆操作\ncond=>condition: 登陆成功 Yes or No?\ne=>end: 进入后台\n\nst->op->cond\ncond(yes)->e\ncond(no)->op\n";
    #[test]
    fn chinese_branches_and_return_path_render_and_edit() {
        let graph = parse(FLOW).unwrap();
        assert_eq!((graph.nodes.len(), graph.edges.len()), (4, 4));
        assert_eq!(ranks(&graph), vec![0, 1, 2, 3]);
        let svg = svg("flow", FLOW).unwrap();
        assert!(svg.contains("用户登陆"));
        assert_eq!(svg.matches("marker-end").count(), 4);
        assert!(svg.contains(">yes</text>") && svg.contains(">no</text>"));
        let a = render("flow", FLOW).unwrap();
        let b = render("flow", &FLOW.replace("用户登陆", "提交订单")).unwrap();
        assert!(a.png.len() > 4000);
        assert_ne!(a.id, b.id);
        assert_eq!(render("flow", FLOW).unwrap().id, a.id);
    }
    #[test]
    fn joins_cycles_and_xml_are_bounded() {
        let source = "a=>condition: A < B & C\nb=>operation: yes branch\nc=>operation: no branch\nd=>end: done\na(yes)->b->d\na(no)->c->d\nd->a";
        let graph = parse(source).unwrap();
        assert_eq!(ranks(&graph), vec![0, 1, 1, 2]);
        let image = svg("flowchart", source).unwrap();
        assert!(image.contains("&lt;") && image.contains("&amp;"));
        assert!(render("flow", "a=>operation: loop\na->a").is_ok());
        assert!(
            render("flow", "a=>start: A\na->missing")
                .unwrap_err()
                .contains("Unknown node")
        );
        assert!(render("flow", "a=>start: A\na=>end: B").is_err());
        assert!(render("flow", "a=>evil: A").is_err());
        assert!(render("flow", &"x".repeat(MAX_SOURCE + 1)).is_err());
    }

    #[test]
    fn document_hosts_render_flow_without_rewriting_source() {
        let source = format!("```flow\n{FLOW}```\n");
        let images = crate::content::Images::default();
        let mut native = crate::content::NativeRenderer {
            images: &images,
            size: 14.,
            ink: 0x191919,
        };
        let native_html = makepad_markdown::markdown_render::render(&source, &mut native)
            .iter()
            .map(|b| b.html.as_str())
            .collect::<String>();
        assert!(native_html.contains("<rdiagram>"));
        assert!(!native_html.contains("<rcode>"));
        let mut granted = Vec::new();
        let mut html = crate::content::HtmlRenderer {
            images: &images,
            size: 14.,
            ink: 0x191919,
            error: None,
            math_count: 0,
            register: |id: &str, bytes: &[u8]| {
                granted.push((id.to_owned(), bytes.to_vec()));
                Ok(id.to_owned())
            },
        };
        let markup = makepad_markdown::markdown_render::render(&source, &mut html)
            .iter()
            .map(|b| b.html.as_str())
            .collect::<String>();
        assert!(html.error.is_none());
        assert!(markup.contains("class=\"diagram\""));
        assert_eq!(granted.len(), 1);
        assert_eq!(
            granted[0].1.as_slice(),
            render("flow", FLOW).unwrap().png.as_slice()
        );
    }
}
