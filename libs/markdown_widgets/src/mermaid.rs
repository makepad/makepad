//! Rust-only Mermaid layout, shared by native and HTML hosts.
use crate::sequence::{self, Head, Sequence};
use makepad_markdown::escape;
use mermaid_rs_renderer::{DiagramKind, Graph, LayoutConfig, Theme};

pub(crate) fn svg(source: &str) -> Result<String, String> {
    if source.lines().count() > 512 {
        return Err("Diagram exceeds 512 lines".into());
    }
    // Validate balanced frames/subgraphs before the layout engine sees them.
    match mermaid_rs_renderer::validator::validate(source) {
        Ok(()) | Err(mermaid_rs_renderer::ParseError::UnknownParticipant { .. }) => {}
        Err(error) => return Err(error.to_string()),
    }
    let parsed = mermaid_rs_renderer::parse_mermaid(source).map_err(|e| e.to_string())?;
    if parsed.graph.kind == DiagramKind::Sequence {
        return render(sequence::mermaid(source)?);
    }
    render(Sequence {
        graph: parsed.graph,
        heads: Vec::new(),
        title: None,
    })
}

pub(crate) fn sequence_svg(source: &str) -> Result<String, String> {
    render(sequence::editor(source)?)
}

fn render(sequence: Sequence) -> Result<String, String> {
    let Sequence {
        graph,
        heads,
        title,
    } = sequence;
    bounds(&graph)?;
    let mut theme = Theme::mermaid_default();
    theme.font_family = "Arial, PingFang SC, Noto Sans CJK SC, sans-serif".into();
    let config = LayoutConfig::default();
    let layout = mermaid_rs_renderer::compute_layout(&graph, &theme, &config);
    if let mermaid_rs_renderer::layout::DiagramData::Error(error) = &layout.diagram {
        return Err(format!("Diagram layout failed: {error:?}"));
    }
    let size = mermaid_rs_renderer::measure_svg_dimensions(&layout, &config, None);
    if !size.width.is_finite()
        || !size.height.is_finite()
        || size.width > 2048.
        || size.height > 2048.
    {
        return Err("Diagram exceeds 2048 pixels; split it into smaller diagrams".into());
    }
    let mut svg = mermaid_rs_renderer::render_svg(&layout, &theme, &config);
    if !heads.is_empty() {
        svg = open_arrows(&svg, &heads)?;
    }
    if let Some(title) = title.filter(|s| !s.is_empty()) {
        // Nest the SVG to retain its own viewBox and coordinates. Titles remain
        // text, never markup, and are measured with the same native font path.
        let lines: Vec<_> = title.lines().collect();
        let extra = lines.len() as f32 * 24. + 16.;
        let width = size
            .width
            .max(title.chars().count().min(100) as f32 * 10. + 32.);
        let height = size.height + extra;
        if height > 2048. {
            return Err("Diagram title exceeds the image size limit".into());
        }
        let inner = svg.replacen(
            "<svg ",
            &format!("<svg y=\"{extra}\" x=\"{}\" ", (width - size.width) / 2.),
            1,
        );
        let text = lines
            .iter()
            .enumerate()
            .map(|(i, line)| {
                format!(
                    "<text x=\"{}\" y=\"{}\" text-anchor=\"middle\">{}</text>",
                    width / 2.,
                    24 + i * 24,
                    escape(line)
                )
            })
            .collect::<String>();
        svg = format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\"><rect width=\"100%\" height=\"100%\" fill=\"white\"/><g font-family=\"{}\" font-size=\"18\">{text}</g>{inner}</svg>",
            escape(&theme.font_family)
        );
    }
    Ok(svg)
}

fn bounds(graph: &Graph) -> Result<(), String> {
    if graph.nodes.len() > 64
        || graph.edges.len() > 128
        || graph.subgraphs.len() > 32
        || graph.sequence_notes.len() > 64
        || graph.sequence_frames.len() > 32
        || graph.sequence_boxes.len() > 32
        || graph.sequence_activations.len() > 128
    {
        return Err("Diagram exceeds 64 nodes, 128 messages/connections, or 32 groups".into());
    }
    if graph.nodes.values().any(|n| n.label.len() > 2048)
        || graph
            .edges
            .iter()
            .any(|e| e.label.as_ref().is_some_and(|l| l.len() > 2048))
    {
        return Err("Diagram label exceeds 2 KB".into());
    }
    Ok(())
}

/// The pinned renderer draws all sequence arrowheads filled. Replace only the
/// generated message path's marker attribute, using its parsed edge index.
/// This is a renderer compatibility fix, independent of any document content.
fn open_arrows(svg: &str, heads: &[Head]) -> Result<String, String> {
    let doc = roxmltree::Document::parse(svg).map_err(|e| e.to_string())?;
    let mut edits = Vec::new();
    let mut defs = String::new();
    for (index, head) in heads.iter().enumerate() {
        if *head != Head::Open {
            continue;
        }
        let id = format!("edge-{index}");
        let node = doc
            .descendants()
            .find(|n| n.has_tag_name("path") && n.attribute("id") == Some(id.as_str()))
            .ok_or("Sequence renderer omitted a message path")?;
        let marker = node
            .attributes()
            .find(|a| a.name() == "marker-end")
            .ok_or("Sequence renderer omitted an arrowhead")?;
        let color = escape(node.attribute("stroke").unwrap_or("#333"));
        defs.push_str(&format!("<marker id=\"makepad-open-{index}\" viewBox=\"0 0 10 10\" refX=\"9\" refY=\"5\" markerWidth=\"8\" markerHeight=\"8\" orient=\"auto\"><path d=\"M 1 1 L 9 5 L 1 9\" fill=\"none\" stroke=\"{color}\" stroke-width=\"1.4\"/></marker>"));
        edits.push((
            marker.range(),
            format!("marker-end=\"url(#makepad-open-{index})\""),
        ));
    }
    edits.sort_by_key(|e| std::cmp::Reverse(e.0.start));
    let mut output = svg.to_owned();
    for (range, replacement) in edits {
        output.replace_range(range, &replacement);
    }
    if !defs.is_empty() {
        let end = output.rfind("</svg>").ok_or("Missing SVG root")?;
        output.insert_str(end, &format!("<defs>{defs}</defs>"));
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagram;
    const SEQ: &str = "Andrew->China: Says Hello\nNote right of China: China thinks\\nabout it\nChina-->Andrew: How are you?\nAndrew->>China: I am good thanks!";

    #[test]
    fn exact_editor_sample_renders_arrows_notes_and_source_edits() {
        let svg = sequence_svg(SEQ).unwrap();
        assert!(svg.contains("makepad-open-2"));
        let doc = roxmltree::Document::parse(&svg).unwrap();
        assert!(
            doc.descendants()
                .any(|n| n.text().is_some_and(|t| t.contains("about it")))
        );
        let a = diagram::render("seq", SEQ).unwrap();
        let b = diagram::render("seq", &SEQ.replace("Says Hello", "你好世界")).unwrap();
        assert!(a.png.len() > 4000);
        assert_ne!(a.id, b.id);
        assert_eq!(a.id, diagram::render("sequence", SEQ).unwrap().id);
    }

    #[test]
    fn sequence_svg_preserves_arrow_semantics_and_title() {
        let source = "sequenceDiagram\nA->B: line\nA-->B: dashed\nA->>B: filled\nA-)B: open\nA--xB: cross\nB<<-->>A: both";
        let svg = svg(source).unwrap();
        let doc = roxmltree::Document::parse(&svg).unwrap();
        let path = |id| {
            doc.descendants()
                .find(|n| n.has_tag_name("path") && n.attribute("id") == Some(id))
                .unwrap()
        };
        assert!(path("edge-0").attribute("marker-end").is_none());
        assert!(path("edge-1").attribute("stroke-dasharray").is_some());
        assert!(path("edge-2").attribute("marker-end").is_some());
        assert_eq!(
            path("edge-3").attribute("marker-end"),
            Some("url(#makepad-open-3)")
        );
        assert!(path("edge-4").attribute("marker-end").is_none());
        assert!(path("edge-5").attribute("marker-start").is_some());
        assert!(
            diagram::render(
                "seq",
                "Title: 测试\nparticipant 客户 as C\nC->C: 自己\\n处理"
            )
            .is_ok()
        );
    }

    #[test]
    fn mermaid_families_render_native_pixels() {
        for source in [
            "flowchart TD\nA[用户登录] --> B{通过?}\nB -->|Yes| C[完成]\nB -->|No| A",
            "sequenceDiagram\nparticipant A as 客户端\nparticipant B as 服务端\nloop 重试\nA->>+B: 请求\nalt 成功\nB-->>-A: 完成\nelse 错误\nB-->>A: 重试\nend\nend",
            "classDiagram\nAnimal <|-- Duck\nAnimal : +int age\nDuck : +swim()",
            "stateDiagram-v2\n[*] --> Idle\nIdle --> Active\nActive --> [*]",
            "erDiagram\nCUSTOMER ||--o{ ORDER : places",
            "pie title Pets\n\"Cats\" : 4\n\"Dogs\" : 6",
            "gantt\ntitle Project\ndateFormat YYYY-MM-DD\nsection Build\nNative :a1, 2026-09-22, 2d",
            "mindmap\n  root((Markdown))\n    Markdown\n    Diagrams",
            "gitGraph\ncommit\nbranch develop\ncheckout develop\ncommit\ncheckout main\nmerge develop",
            "timeline\ntitle History\n2025 : Start\n2026 : Native",
        ] {
            let image =
                diagram::render("mermaid", source).unwrap_or_else(|e| panic!("{source}\n{e}"));
            assert!(
                image.width > 20 && image.height > 20 && image.png.len() > 500,
                "{source}"
            );
        }
    }

    #[test]
    fn native_and_html_hosts_render_diagrams_from_source() {
        let source = format!(
            "# Diagrams\n\n```seq\n{SEQ}\n```\n\n```mermaid\nflowchart LR\nA[你好]-->B[Done]\n```\n\n```mermaid\ninvalid input\n```\n"
        );
        let images = crate::content::Images::default();
        let mut native = crate::content::NativeRenderer {
            images: &images,
            size: 14.,
            ink: 0x191919,
        };
        let blocks = makepad_markdown::markdown_render::render(&source, &mut native);
        let markup = blocks.iter().map(|b| b.html.as_str()).collect::<String>();
        assert_eq!(markup.matches("<rdiagram>").count(), 2);
        assert!(markup.contains("Diagram:") && markup.contains("invalid input"));
        assert!(blocks.iter().any(|b| b.text.contains("Andrew->China")));
        let mut resources = Vec::new();
        let mut html = crate::content::HtmlRenderer {
            images: &images,
            size: 14.,
            ink: 0x191919,
            error: None,
            math_count: 0,
            register: |id: &str, png: &[u8]| {
                resources.push((id.to_owned(), png.to_vec()));
                Ok(id.to_owned())
            },
        };
        let markup = makepad_markdown::markdown_render::render(&source, &mut html)
            .iter()
            .map(|b| b.html.as_str())
            .collect::<String>();
        assert_eq!(markup.matches("class=\"diagram\"").count(), 2);
        assert_eq!(resources.len(), 2);
        assert!(resources.iter().all(|(_, png)| png.starts_with(b"\x89PNG")));
    }

    #[test]
    fn malformed_and_oversized_diagrams_fall_back_with_source() {
        for source in [
            "nonsense",
            "flowchart TD\nsubgraph unclosed\nA-->B",
            "sequenceDiagram\nA->>B: hi\nbogus",
        ] {
            assert!(diagram::render("mermaid", source).is_err(), "{source}");
        }
        let source = format!(
            "flowchart TD\n{}",
            (0..70)
                .map(|i| format!("n{i}-->n{}\n", i + 1))
                .collect::<String>()
        );
        assert!(
            diagram::render("mermaid", &source)
                .unwrap_err()
                .contains("64 nodes")
        );
    }
}
