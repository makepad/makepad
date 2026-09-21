use crate::report::ScriptState;
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    // The tile's draw type: DrawQuad's vertex stage and rect, plus the
    // instance fields the Rust struct below declares. They are instance
    // fields because they are `#[live]` after the `#[deref]`; the script only
    // gives them values.
    set_type_default() do #(DrawSquare::script_shader(vm)){
        ..mod.draw.DrawQuad
    }
    mod.widgets.CiWall = #(CiWall::register_widget(vm)){
        width: Fill height: Fill
        draw_square +: {
            color: #3a3f46
            previous: #0000
            pulse: 1.0
            progress: -1.0
            selected: 0.0
            pixel: fn(){
                let p = self.pos * self.rect_size
                let edge = min(min(p.x, p.y), min(self.rect_size.x-p.x, self.rect_size.y-p.y))
                if edge < 2.0 && self.selected > 0.5 {return #9aa1ab}
                // What the last finished run said, while this run has not got here.
                if self.previous.w > 0.5 && length(p - vec2(self.rect_size.x-15.0, 15.0)) < 6.0 {
                    return vec4(self.previous.xyz, 1.0)
                }
                if self.progress >= 0.0 && p.y > self.rect_size.y-10.0 {
                    if self.pos.x < self.progress {return #8f98a3}
                    return #0009
                }
                return vec4(self.color.xyz * self.pulse, 1.0)
            }
        }
        // A named family: an empty one only renders where a system fallback
        // happens to exist, and the tiles came up blank on the CI box.
        draw_name +: {color: #c9ced6 text_style: theme.font_bold{font_size: 14}}
        draw_text +: {color: #8e96a1 text_style: theme.font_regular{font_size: 11}}
    }
}
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawSquare {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    color: Vec4f,
    #[live]
    previous: Vec4f,
    #[live]
    pulse: f32,
    #[live]
    progress: f32,
    #[live]
    selected: f32,
}
#[derive(Clone)]
pub struct Tile {
    pub key: String,
    /// What is tested, short enough to read across the room: `wm`, `scope`.
    pub label: String,
    pub state: ScriptState,
}
#[derive(Script, ScriptHook, Widget)]
pub struct CiWall {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[redraw]
    #[rust]
    area: Area,
    #[live]
    draw_square: DrawSquare,
    #[live]
    draw_name: DrawText,
    #[live]
    draw_text: DrawText,
    #[rust]
    tiles: Vec<Tile>,
    #[rust]
    rects: Vec<Rect>,
    #[rust]
    selected: Option<String>,
    #[rust]
    pulse_timer: Timer,
    #[rust]
    time: f64,
}
/// The pulse and the elapsed seconds of a running tile advance on this beat.
/// Never per frame: the wall display runs at 240 Hz.
const PULSE_SECONDS: f64 = 0.125;
fn rgb(r: u8, g: u8, b: u8) -> Vec4f {
    vec4(r as f32 / 255., g as f32 / 255., b as f32 / 255., 1.)
}
/// The wall is an OLED that shows this all day, so everything is a dark grey
/// with a hint of its meaning: untested grey, passed grey-green, a warning
/// grey-amber, the one being tested grey-blue. A failure is the ONLY bright
/// thing on the screen.
fn colour(verdict: &str) -> Vec4f {
    match verdict {
        "green" => rgb(40, 66, 52),
        "orange" => rgb(88, 70, 40),
        "red" => rgb(255, 38, 38),
        "running" => rgb(40, 58, 80),
        _ => rgb(34, 37, 42),
    }
}
/// Text is a quiet grey, except on the red tile, where it is white.
fn ink(verdict: &str, name: bool) -> Vec4f {
    match (verdict, name) {
        ("red", _) => rgb(255, 255, 255),
        ("waiting", true) => rgb(132, 139, 149),
        ("waiting", false) => rgb(96, 103, 112),
        (_, true) => rgb(201, 206, 214),
        (_, false) => rgb(142, 150, 161),
    }
}
/// The longest name a tile shows whole; longer ones end in an ellipsis.
const NAME_CHARS: f64 = 12.0;
const NAME_ADVANCE: f64 = 0.64;
/// The name's font size in a tile: as large as the tile's height and a whole
/// NAME_CHARS name allow.
fn name_font(size: DVec2) -> f64 {
    (size.y / 5.0).min((size.x - 34.0) / (NAME_CHARS * NAME_ADVANCE)).clamp(9.0, 24.0)
}
/// Columns and tile size. Every tile fits, fills its column, and is never
/// taller than wide; among those the packing with the LARGEST NAME wins, since
/// the name is what is read from across the room.
pub fn layout(width: f64, height: f64, count: usize) -> (usize, DVec2) {
    if count == 0 || width <= 0.0 || height <= 0.0 {
        return (1, dvec2(0.0, 0.0));
    }
    let mut best = (1, dvec2(0.0, 0.0), (0.0, 0.0));
    for cols in 1..=count {
        let rows = count.div_ceil(cols);
        let w = width / cols as f64;
        let size = dvec2(w, (height / rows as f64).min(w));
        let score = (name_font(size), size.x * size.y);
        if score > best.2 {
            best = (cols, size, score);
        }
    }
    (best.0, best.1)
}
fn ellipsis(line: &str, chars: usize) -> String {
    if line.chars().count() > chars {
        format!("{}…", line.chars().take(chars.saturating_sub(1)).collect::<String>())
    } else {
        line.into()
    }
}
fn clock(seconds: f64) -> String {
    let s = seconds.max(0.0) as u64;
    if s >= 60 { format!("{}m{:02}s", s / 60, s % 60) } else { format!("{s}s") }
}
impl Widget for CiWall {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if let Hit::FingerDown(e) = event.hits(cx, self.area) {
            if let Some(index) = self.rects.iter().position(|r| r.contains(e.abs)) {
                self.selected = Some(self.tiles[index].key.clone());
                self.area.redraw(cx);
            }
        }
        if self.pulse_timer.is_event(event).is_some() {
            self.time += PULSE_SECONDS;
            self.area.redraw(cx);
        }
    }
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let rect = cx.walk_turtle(walk);
        cx.add_rect_area(&mut self.area, rect);
        let (cols, cell) = layout(rect.size.x, rect.size.y, self.tiles.len());
        let gap = (cell.y * 0.05).clamp(3.0, 8.0);
        self.rects.clear();
        self.draw_square.begin_many_instances(cx);
        for (index, tile) in self.tiles.iter().enumerate() {
            let r = Rect {
                pos: rect.pos + dvec2((index % cols) as f64 * cell.x, (index / cols) as f64 * cell.y),
                size: dvec2((cell.x - gap).max(0.0), (cell.y - gap).max(0.0)),
            };
            self.rects.push(r);
            let verdict = tile.state.color_verdict();
            let running = verdict == "running";
            self.draw_square.color = colour(verdict);
            self.draw_square.previous = if matches!(verdict, "waiting" | "running")
                && matches!(tile.state.previous.as_str(), "green" | "orange" | "red")
            {
                colour(&tile.state.previous)
            } else {
                vec4(0., 0., 0., 0.)
            };
            self.draw_square.pulse = if running {
                0.86 + 0.14 * (self.time * 3.0).sin() as f32
            } else {
                1.0
            };
            let (p, w, f) = tile.state.counts();
            self.draw_square.progress = if running {
                ((p + w + f) as f32 / (tile.state.steps.len() + 1) as f32).min(0.98)
            } else {
                -1.0
            };
            self.draw_square.selected = if self.selected.as_ref() == Some(&tile.key) { 1.0 } else { 0.0 };
            self.draw_square.draw_abs(cx, r);
        }
        self.draw_square.end_many_instances(cx);
        let name_font = name_font(cell);
        let font = (name_font * 0.72).clamp(9.0, 15.0);
        self.draw_name.text_style.font_size = name_font as f32;
        self.draw_text.text_style.font_size = font as f32;
        self.draw_name.begin_many_instances(cx);
        for (tile, r) in self.tiles.iter().zip(&self.rects) {
            let chars = ((r.size.x - 34.0 + 6.0) / (name_font * NAME_ADVANCE)).max(1.0) as usize;
            self.draw_name.color = ink(tile.state.color_verdict(), true);
            self.draw_name.draw_abs(cx, r.pos + dvec2(10.0, 8.0), &ellipsis(&tile.label, chars));
        }
        self.draw_name.end_many_instances(cx);
        self.draw_text.begin_many_instances(cx);
        for (tile, r) in self.tiles.iter().zip(&self.rects) {
            // Capitals and digits run wide ("VERDICT: NO"): a line never leaves its tile.
            let chars = ((r.size.x - 24.0) / (font * 0.68)).max(1.0) as usize;
            let state = &tile.state;
            self.draw_text.color = ink(state.color_verdict(), false);
            let (p, w, f) = state.counts();
            let short = |s: &crate::report::Stage| s.name.rsplit(" / ").next().unwrap_or(&s.name).to_string();
            let lines: Vec<String> = match state.color_verdict() {
                "running" => vec![
                    state.steps.iter().rev().find(|s| s.state == "running").map(short).unwrap_or_else(|| "starting".into()),
                    format!("{} done · {}", p + w + f, clock(state.started.map(|t| t.elapsed().as_secs_f64()).unwrap_or(state.seconds))),
                ],
                "red" => vec![
                    state.steps.iter().find(|s| s.state == "failed").map(short).unwrap_or_else(|| "failed".into()),
                    state.detail.lines().next().unwrap_or("").into(),
                    clock(state.seconds),
                ],
                "orange" => vec![
                    format!("{w} warning{}", if w == 1 { "" } else { "s" }),
                    state.detail.lines().next().unwrap_or("").into(),
                    clock(state.seconds),
                ],
                "green" => vec![format!("{p} passed"), clock(state.seconds)],
                _ => vec![match state.previous.as_str() {
                    "red" => "untested · failed last run",
                    "orange" => "untested · warnings last run",
                    "green" => "untested · passed last run",
                    _ => "untested",
                }.into()],
            };
            for (index, line) in lines.iter().filter(|l| !l.is_empty()).enumerate() {
                let y = 10.0 + name_font * 1.55 + index as f64 * (font * 1.5);
                if y + font > r.size.y - 12.0 {
                    break;
                }
                self.draw_text.draw_abs(cx, r.pos + dvec2(10.0, y), &ellipsis(line, chars));
            }
        }
        self.draw_text.end_many_instances(cx);
        DrawStep::done()
    }
}
impl CiWallRef {
    pub fn set_tiles(&self, cx: &mut Cx, tiles: Vec<Tile>, selected: Option<String>) {
        if let Some(mut inner) = self.borrow_mut() {
            let was_running = inner.tiles.iter().any(|t| t.state.verdict == "running");
            let running = tiles.iter().any(|t| t.state.verdict == "running");
            inner.tiles = tiles;
            inner.selected = selected;
            // Nothing running: no timer, no frames. The wall rests.
            if running && !was_running {
                inner.pulse_timer = cx.start_interval(PULSE_SECONDS);
            } else if !running && was_running {
                cx.stop_timer(inner.pulse_timer);
            }
            inner.area.redraw(cx);
        }
    }
    pub fn selected(&self) -> Option<String> {
        self.borrow().and_then(|inner| inner.selected.clone())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tiles_fit_and_keep_a_readable_shape() {
        for count in [1, 2, 3, 8, 30, 50, 101] {
            let (cols, size) = layout(936.0, 610.0, count);
            assert!(cols as f64 * size.x <= 936.001, "{count}: too wide");
            assert!(count.div_ceil(cols) as f64 * size.y <= 610.001, "{count}: too tall");
            assert!(size.x + 0.001 >= size.y, "{count}: taller than wide {size:?}");
        }
        assert_eq!(layout(0.0, 100.0, 4).1, dvec2(0.0, 0.0));
        // Thirty scripts on the wall display: wide tiles, so `calculator` is whole.
        let (cols, size) = layout(936.0, 500.0, 32);
        assert_eq!(cols, 6);
        assert!(name_font(size) >= 14.0, "{size:?}");
    }
    #[test]
    fn untested_is_grey_and_never_the_last_runs_colour() {
        assert_eq!(colour("waiting"), colour("anything else"));
        for verdict in ["green", "orange", "red", "running"] {
            assert_ne!(colour(verdict), colour("waiting"));
        }
        // Only a failure is bright: every other tile stays under a third of full.
        for verdict in ["green", "orange", "running", "waiting"] {
            let c = colour(verdict);
            assert!(c.x.max(c.y).max(c.z) < 0.36, "{verdict} is too bright for the OLED");
        }
        assert!(colour("red").x > 0.95);
        assert_eq!(clock(59.9), "59s");
        assert_eq!(clock(125.0), "2m05s");
    }
}
