//! The inspect client's text answers: inspect's own commands go through the one renderer
//! ([`makepad_prof_stream::answer`], which Scope's Live console calls too); the app's
//! remote-control commands (`snap`, `dump`, `click`, ..) keep their own output.

use makepad_prof_stream::answer::{self, Request};

/// Whether `line` asks one of inspect's own commands (the JIT's and the GPU tracer's).
fn renders(line: &str) -> bool {
    let req = makepad_remote::Req::parse(line);
    let c = req.cmd.as_str();
    // (`src`, `vm`, `verbs`, `script`: the Splash VM's host commands, makepad_script::inspect_query;
    // `game`: the Sandbox's app query)
    matches!(c, "frames" | "frame" | "top" | "path" | "hang" | "panics" | "resume" | "rm" | "continue" | "pause" | "src" | "vm" | "verbs" | "script" | "game")
        || super::COMMANDS.iter().chain(makepad_gpu_trace::query::COMMANDS).any(|x| x.0 == c)
}

/// Renders the answer to request `line` from its JSON lines (the `end` line left out); lines of
/// a command inspect does not own come back unchanged.
pub fn render(line: &str, lines: &[String]) -> String {
    if !renders(line) {
        return lines.iter().map(|l| format!("{}\n", l)).collect();
    }
    let req = makepad_remote::Req::parse(line);
    answer::render(&Request { cmd: req.cmd, pos: req.pos, kv: req.kv }, lines)
}

/// One streamed line (`events follow=1`, `watch`, `all`): the line's own short text.
pub fn render_streamed(line: &str, l: &str) -> String {
    let out = render(line, &[l.to_string()]);
    out.trim_end().to_string()
}
