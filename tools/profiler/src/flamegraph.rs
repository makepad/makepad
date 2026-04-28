use crate::trace_file::{FoldedStack, TraceFile};
use makepad_profiler::ProfilerError;
use std::collections::BTreeMap;
use std::fmt::Write;
use std::path::Path;

pub struct FlamegraphOptions {
    pub title: String,
    pub include_blocked: bool,
    pub width: usize,
}

pub fn write_flamegraph(
    trace: &TraceFile,
    path: &Path,
    options: &FlamegraphOptions,
) -> Result<(), ProfilerError> {
    let stacks = trace.folded_stacks(options.include_blocked);
    if stacks.is_empty() {
        return Err(ProfilerError::new(
            "trace did not contain any stack samples eligible for flamegraph rendering",
        ));
    }

    let svg = render_flamegraph(&stacks, options);
    std::fs::write(path, svg).map_err(|err| {
        ProfilerError::new(format!(
            "failed to write flamegraph {}: {}",
            path.display(),
            err
        ))
    })
}

#[derive(Default)]
struct FlameNode {
    name: String,
    weight: u64,
    children: BTreeMap<String, FlameNode>,
}

impl FlameNode {
    fn new(name: String) -> Self {
        Self {
            name,
            weight: 0,
            children: BTreeMap::new(),
        }
    }

    fn insert(&mut self, frames: &[String], weight: u64) {
        self.weight += weight;
        if let Some((head, tail)) = frames.split_first() {
            self.children
                .entry(head.clone())
                .or_insert_with(|| FlameNode::new(head.clone()))
                .insert(tail, weight);
        }
    }

    fn max_depth(&self) -> usize {
        self.children
            .values()
            .map(|child| 1 + child.max_depth())
            .max()
            .unwrap_or(0)
    }
}

fn render_flamegraph(stacks: &[FoldedStack], options: &FlamegraphOptions) -> String {
    let mut root = FlameNode::default();
    for stack in stacks {
        root.insert(&stack.frames, stack.weight);
    }

    let graph_width = options.width.max(400) as f64;
    let side_padding = 12.0;
    let top_padding = 42.0;
    let bottom_padding = 24.0;
    let frame_height = 18.0;
    let depth = root.max_depth().max(1);
    let canvas_width = graph_width + side_padding * 2.0;
    let canvas_height = top_padding + bottom_padding + depth as f64 * frame_height;
    let scale = graph_width / root.weight.max(1) as f64;

    let mut svg = String::new();
    let _ = writeln!(
        svg,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {:.0} {:.0}\" width=\"{:.0}\" height=\"{:.0}\" font-family=\"Menlo, Monaco, monospace\" font-size=\"12\">",
        canvas_width,
        canvas_height,
        canvas_width,
        canvas_height
    );
    let _ = writeln!(
        svg,
        "<rect x=\"0\" y=\"0\" width=\"{:.0}\" height=\"{:.0}\" fill=\"#fff7ef\" />",
        canvas_width,
        canvas_height
    );
    let _ = writeln!(
        svg,
        "<text x=\"{:.1}\" y=\"24\" fill=\"#3f3223\" font-size=\"16\">{}</text>",
        side_padding,
        escape_xml(&options.title)
    );
    let _ = writeln!(
        svg,
        "<text x=\"{:.1}\" y=\"38\" fill=\"#7c6753\" font-size=\"11\">{}</text>",
        side_padding,
        escape_xml(&format!(
            "{} total ms sampled{}",
            root.weight as f64 / 1_000.0,
            if options.include_blocked {
                " (including blocked stacks)"
            } else {
                ""
            }
        ))
    );

    let mut x = side_padding;
    for child in root.children.values() {
        let width = child.weight as f64 * scale;
        render_node(
            &mut svg,
            child,
            x,
            0,
            width,
            depth,
            top_padding,
            frame_height,
        );
        x += width;
    }

    svg.push_str("</svg>\n");
    svg
}

fn render_node(
    out: &mut String,
    node: &FlameNode,
    x: f64,
    depth: usize,
    width: f64,
    max_depth: usize,
    top_padding: f64,
    frame_height: f64,
) {
    if width < 0.5 {
        return;
    }

    let y = top_padding + (max_depth.saturating_sub(depth + 1)) as f64 * frame_height;
    let (fill, stroke, text_fill) = node_colors(&node.name);
    let label = clipped_text(&node.name, width - 6.0);
    let _ = writeln!(
        out,
        "<g><title>{}</title><rect x=\"{:.2}\" y=\"{:.2}\" width=\"{:.2}\" height=\"{:.2}\" rx=\"2\" ry=\"2\" fill=\"{}\" stroke=\"{}\" stroke-width=\"0.5\" />",
        escape_xml(&format!("{} ({:.3} ms)", node.name, node.weight as f64 / 1_000.0)),
        x,
        y,
        width.max(0.0),
        frame_height - 1.0,
        fill,
        stroke
    );
    if !label.is_empty() {
        let _ = writeln!(
            out,
            "<text x=\"{:.2}\" y=\"{:.2}\" fill=\"{}\">{}</text>",
            x + 3.0,
            y + frame_height - 5.0,
            text_fill,
            escape_xml(&label)
        );
    }
    out.push_str("</g>\n");

    let mut child_x = x;
    for child in node.children.values() {
        let child_width = child.weight as f64 * width / node.weight.max(1) as f64;
        render_node(
            out,
            child,
            child_x,
            depth + 1,
            child_width,
            max_depth,
            top_padding,
            frame_height,
        );
        child_x += child_width;
    }
}

fn clipped_text(text: &str, available_width: f64) -> String {
    if available_width < 12.0 {
        return String::new();
    }
    let max_chars = (available_width / 7.0).floor() as usize;
    let char_count = text.chars().count();
    if char_count <= max_chars {
        return text.to_string();
    }
    if max_chars <= 3 {
        return String::new();
    }
    let mut clipped = String::new();
    for ch in text.chars().take(max_chars - 3) {
        clipped.push(ch);
    }
    clipped.push_str("...");
    clipped
}

fn node_colors(name: &str) -> (&'static str, &'static str, &'static str) {
    if name == "[blocked]" {
        return ("#cfc6bf", "#8d7f73", "#3f3223");
    }

    match fnv1a64(name.as_bytes()) % 6 {
        0 => ("#f7b267", "#d97706", "#3f3223"),
        1 => ("#f79d84", "#dd6b4d", "#3f3223"),
        2 => ("#f48498", "#d14d72", "#3f3223"),
        3 => ("#9cd08f", "#5f9e55", "#24361f"),
        4 => ("#8cc7c3", "#4f938d", "#1f3735"),
        _ => ("#aab4f7", "#6974d8", "#27305c"),
    }
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in bytes {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    hash
}

fn escape_xml(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            _ => escaped.push(ch),
        }
    }
    escaped
}
