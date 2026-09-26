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
            color: #x3a3f46
            pulse: 1.0
            progress: -1.0
            selected: 0.0
            pixel: fn(){
                let p = self.pos * self.rect_size
                let sdf = Sdf2d.viewport(p)
                sdf.box(1.0, 1.0, self.rect_size.x-2.0, self.rect_size.y-2.0, 4.0)
                let mut fill = vec4(self.color.xyz * self.pulse, 1.0)
                if self.progress >= 0.0 && p.y > self.rect_size.y-5.0 {
                    fill = #x1c2229
                    if p.x < 8.0 + (self.rect_size.x-16.0) * self.progress {fill = #x788491}
                }
                sdf.fill_keep(fill)
                if self.selected > 0.5 {sdf.stroke(#x899098, 1.0)}
                return sdf.result
            }
        }
        // A named family: an empty one only renders where a system fallback
        // happens to exist, and the tiles came up blank on the CI box.
        scroll_bars: mod.widgets.ScrollBars{show_scroll_x: false show_scroll_y: true}
        draw_name +: {color: #c9ced6 text_style: theme.font_bold{font_size: 14}}
        draw_text +: {color: #x8e96a1 text_style: theme.font_regular{font_size: 11}}
    }
    mod.widgets.CiBranches = #(CiBranches::register_widget(vm)){
        width: Fill height: Fit
        draw_name +: {color: #b7bec7 text_style: theme.font_bold{font_size: 22}}
        draw_progress +: {color: #b7bec7 text_style: theme.font_bold{font_size: 30}}
        draw_tip +: {color: #75808d text_style: theme.font_code{font_size: 11}}
        draw_state +: {color: #a0aab6 text_style: theme.font_regular{font_size: 13}}
        draw_age +: {color: #x7b8590 text_style: theme.font_regular{font_size: 11}}
        draw_problem +: {color: #x626b76 text_style: theme.font_regular{font_size: 10}}
    }

    mod.widgets.CiSteps = #(CiSteps::register_widget(vm)){
        width: Fill height: Fill
        scroll_bars: mod.widgets.ScrollBars{show_scroll_x: false show_scroll_y: true}
        draw_block +: {
            color: #x080b0e pulse: 1.0 progress: -1.0 selected: 0.0
            pixel: fn(){
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(0.0, 0.0, self.rect_size.x, self.rect_size.y, 3.0)
                sdf.fill(self.color)
                return sdf.result
            }
        }
        draw_name +: {color: #959faa text_style: theme.font_regular{font_size: 11}}
        draw_time +: {color: #697480 text_style: theme.font_code{font_size: 10}}
        draw_reason +: {color: #8d97a3 text_style: theme.font_code{font_size: 10}}
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
    draw_list: DrawList2d,
    #[live]
    scroll_bars: ScrollBars,
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
    (size.y / 4.0).min((size.x - 20.0) / (NAME_CHARS * NAME_ADVANCE)).clamp(9.0, 24.0)
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
/// Text shaping supplies actual advances, including fallback and wide glyphs.
fn ellipsis(draw: &DrawText, cx: &mut Cx, line: &str, width: f64) -> String {
    let measure = |cx: &mut Cx, text: &str| draw.layout(cx, 0.0, 0.0, None, false, Align::default(), text).size_in_lpxs.width as f64;
    if measure(cx, line) <= width { return line.into(); }
    let mut ends: Vec<usize> = line.char_indices().map(|(i, _)| i).collect();
    ends.push(line.len());
    let (mut low, mut high) = (0, ends.len()-1);
    while low < high {
        let mid = (low+high+1)/2;
        if measure(cx, &format!("{}…", &line[..ends[mid]])) <= width { low=mid; } else { high=mid-1; }
    }
    format!("{}…", &line[..ends[low]])
}
fn clock(seconds: f64) -> String {
    let s = seconds.max(0.0) as u64;
    if s >= 60 { format!("{}m{:02}s", s / 60, s % 60) } else { format!("{s}s") }
}
impl Widget for CiWall {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if !self.scroll_bars.handle_event(cx, event, scope).is_empty() { self.area.redraw(cx); }
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
        // Keep the wall's batched geometry and text in one owned recording.
        self.draw_list.begin_always(cx);
        self.scroll_bars.begin(cx, walk, Layout::flow_down());
        let viewport = cx.turtle().rect();
        let width = viewport.size.x - 8.0;
        // Keep the existing fit-to-height packing for normal walls, but give
        // dense walls a readable virtual surface that the scrollbars expose.
        let min_height = if self.tiles.len() > 40 { 950.0 } else { viewport.size.y };
        let (cols, mut cell) = layout(width, min_height, self.tiles.len());
        let preferred_cols = ((width / 150.0).floor() as usize).max(1).min(self.tiles.len().max(1));
        let mut cols = cols.min(preferred_cols);
        let mut best = f64::MAX;
        for candidate in 1..=preferred_cols {
            let rows = self.tiles.len().div_ceil(candidate).max(1);
            let height = (min_height / rows as f64).clamp(84.0, 192.0);
            let aspect = (width / candidate as f64 - 8.0) / (height - 8.0);
            let score = (aspect / (5.0/3.0)).ln().abs();
            if score < best {best=score;cols=candidate;}
        }
        cell.x = width / cols as f64;
        let rows = self.tiles.len().div_ceil(cols).max(1);
        cell.y = if self.tiles.len() <= 40 { (viewport.size.y / rows as f64).min(cell.x * 0.64).clamp(84.0, if self.tiles.len() <= 8 {192.0} else {160.0}) } else { (cell.x * 0.64).clamp(96.0, 160.0) };
        let gap = 8.0;
        // No taller than its content: the name and two lines under it, with
        // the padding above and below. The time sits beside the count now, so
        // there is no bottom line to keep room for.
        let name_w = ((cell.x - 20.0) / (NAME_CHARS * NAME_ADVANCE)).clamp(13.0, 24.0);
        let body_w = (name_w * 0.66).clamp(10.0, 12.0);
        let content = 12.0 + name_w * 1.33 + 2.0 * body_w * 1.4 + 12.0 + gap;
        cell.y = cell.y.min(content);
        let height = self.tiles.len().div_ceil(cols) as f64 * cell.y;
        let rect = cx.walk_turtle(Walk::fixed(width, height));
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
            self.draw_square.pulse = if running {
                0.86 + 0.14 * (self.time * 3.0).sin() as f32
            } else {
                1.0
            };
            let (p, w, f) = tile.state.counts();
            self.draw_square.progress = if running {
                ((p + w + f) as f32 / tile.state.steps.len().max(1) as f32).min(0.98)
            } else {
                -1.0
            };
            self.draw_square.selected = if self.selected.as_ref() == Some(&tile.key) { 1.0 } else { 0.0 };
            self.draw_square.draw_abs(cx, r);
        }
        self.draw_square.end_many_instances(cx);
        let name_font = name_font(cell).clamp(13.0, 24.0);
        let font = (name_font * 0.66).clamp(10.0, 12.0);
        self.draw_name.text_style.font_size = name_font as f32;
        self.draw_text.text_style.font_size = font as f32;
        self.draw_name.begin_many_instances(cx);
        for (tile, r) in self.tiles.iter().zip(&self.rects) {
            self.draw_name.color = ink(tile.state.color_verdict(), true);
            self.draw_name.text_style.font_size = name_font as f32;
            while self.draw_name.text_style.font_size > 13.0 && self.draw_name.layout(cx, 0.0, 0.0, None, false, Align::default(), &tile.label).size_in_lpxs.width as f64 > r.size.x-24.0 {
                self.draw_name.text_style.font_size = (self.draw_name.text_style.font_size-1.0).max(13.0);
            }
            let label = ellipsis(&self.draw_name, cx, &tile.label, r.size.x-24.0);
            self.draw_name.draw_abs(cx, r.pos + dvec2(12.0, 10.0), &label);
        }
        self.draw_name.end_many_instances(cx);
        self.draw_text.begin_many_instances(cx);
        for (tile, r) in self.tiles.iter().zip(&self.rects) {
            let state = &tile.state;
            self.draw_text.color = ink(state.color_verdict(), false);
            let (p, w, f) = state.counts();
            let short = |s: &crate::report::Stage| s.name.rsplit(" / ").next().unwrap_or(&s.name).to_string();
            let lines: Vec<String> = match state.color_verdict() {
                "running" => {
                    let step = state.steps.iter().rev().find(|s| s.state == "running");
                    vec![
                        step.map(short).unwrap_or_else(|| "starting".into()),
                        // What a long command is doing, so a ten-minute build moves.
                        step.map(|s| s.progress.clone()).unwrap_or_default(),
                        format!("{} done · {}", p + w + f, clock(state.started.map(|t| t.elapsed().as_secs_f64()).unwrap_or(state.seconds))),
                    ]
                }
                // The time sits beside the count: one line fewer on every card.
                "red" => vec![
                    format!(
                        "{} · {}",
                        state.steps.iter().find(|s| s.state == "failed").map(short).unwrap_or_else(|| "failed".into()),
                        clock(state.seconds)
                    ),
                    state.detail.lines().next().unwrap_or("").trim_start_matches("VERDICT: NO - ").into(),
                ],
                "orange" => vec![
                    format!("{w} warning{} · {}", if w == 1 { "" } else { "s" }, clock(state.seconds)),
                    state.detail.lines().next().unwrap_or("").into(),
                ],
                "green" => vec![format!("{p} passed · {}", clock(state.seconds))],
                // Untested is grey and says so; what an earlier run said is
                // history, told in the detail, never on the wall.
                _ => vec!["untested".into()],
            };
            let lines: Vec<&String> = lines.iter().filter(|l| !l.is_empty()).collect();
            for (index, line) in lines.iter().enumerate() {
                let y = 12.0 + name_font * 1.33 + index as f64 * (font * 1.4);
                if y + font > r.size.y - 12.0 {
                    break;
                }
                let line = ellipsis(&self.draw_text, cx, line, r.size.x-24.0);
                self.draw_text.draw_abs(cx, r.pos + dvec2(12.0, y), &line);
            }
        }
        self.draw_text.end_many_instances(cx);
        self.scroll_bars.end(cx);
        cx.add_rect_area(&mut self.area, viewport);
        self.draw_list.end(cx);
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
pub struct BranchLine {
    pub name: String,
    pub tip: String,
    pub progress: String,
    pub failed: usize,
    pub warned: usize,
    pub state: String,
    pub age: String,
}
#[derive(Script, ScriptHook, Widget)]
pub struct CiBranches {
    #[uid] uid: WidgetUid,
    #[source] source: ScriptObjectRef,
    #[walk] walk: Walk,
    #[redraw] #[rust] area: Area,
    #[live] draw_name: DrawText,
    #[live] draw_tip: DrawText,
    #[live] draw_progress: DrawText,
    #[live] draw_state: DrawText,
    #[live] draw_age: DrawText,
    #[live] draw_problem: DrawText,
    #[rust] branches: Vec<BranchLine>,
    #[rust] problem: String,
}
impl Widget for CiBranches {
    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let rect = cx.walk_turtle(Walk{height: Size::Fixed(58.0*self.branches.len().max(1) as f64), ..walk});
        cx.add_rect_area(&mut self.area, rect);
        if self.branches.is_empty() && !self.problem.is_empty() {
            let text = ellipsis(&self.draw_problem,cx,&self.problem,rect.size.x);
            self.draw_problem.draw_abs(cx,rect.pos+dvec2(0.0,42.0),&text);
        }
        for (i, branch) in self.branches.iter().enumerate() {
            let pos = rect.pos+dvec2(0.0, 58.0*i as f64);
            let progress_x = (rect.size.x * 0.36).min(360.0);
            let name = ellipsis(&self.draw_name, cx, &branch.name, (progress_x-110.0).max(50.0));
            let width = self.draw_name.layout(cx,0.0,0.0,None,false,Align::default(),&name).size_in_lpxs.width as f64;
            let problem = i == 0 && !self.problem.is_empty();
            let name_y = if problem {4.0} else {11.0};
            self.draw_name.draw_abs(cx,pos+dvec2(0.0,name_y),&name);
            self.draw_tip.draw_abs(cx,pos+dvec2(width+16.0,name_y+10.0),&branch.tip);
            if problem {
                let text = ellipsis(&self.draw_problem,cx,&self.problem,(progress_x-18.0).max(0.0));
                self.draw_problem.draw_abs(cx,pos+dvec2(0.0,42.0),&text);
            }
            self.draw_progress.draw_abs(cx,pos+dvec2(progress_x,4.0),&branch.progress);
            let width = self.draw_progress.layout(cx,0.0,0.0,None,false,Align::default(),&branch.progress).size_in_lpxs.width as f64;
            let x = progress_x+width+20.0;
            let available = (rect.size.x-x).max(0.0);
            let mut counts_x = x;
            if branch.failed > 0 {
                self.draw_state.color = colour("red");
                let text = format!("{} failed", branch.failed);
                self.draw_state.draw_abs(cx,pos+dvec2(counts_x,8.0),&text);
                counts_x += self.draw_state.layout(cx,0.0,0.0,None,false,Align::default(),&text).size_in_lpxs.width as f64 + 14.0;
            }
            if branch.warned > 0 {
                self.draw_state.color = rgb(142,150,161);
                let text = format!("{} warning{}", branch.warned, if branch.warned == 1 {""} else {"s"});
                let text = ellipsis(&self.draw_state,cx,&text,(rect.size.x-counts_x).max(0.0));
                self.draw_state.draw_abs(cx,pos+dvec2(counts_x,8.0),&text);
            }
            let status = if branch.state.is_empty() {branch.age.clone()} else if branch.age.is_empty() {branch.state.clone()} else {format!("{} · {}",branch.state,branch.age)};
            let y = if branch.failed > 0 || branch.warned > 0 {33.0} else {22.0};
            let status = ellipsis(&self.draw_age,cx,&status,available);
            self.draw_age.draw_abs(cx,pos+dvec2(x,y),&status);
        }
        DrawStep::done()
    }
}
impl CiBranchesRef {
    pub fn set_branches(&self, cx: &mut Cx, branches: Vec<BranchLine>, problem: String) {
        if let Some(mut inner)=self.borrow_mut() {inner.branches=branches;inner.problem=problem;inner.area.redraw(cx);}
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct CiSteps {
    #[uid] uid: WidgetUid,
    #[source] source: ScriptObjectRef,
    #[walk] walk: Walk,
    #[redraw] #[rust] area: Area,
    #[live] scroll_bars: ScrollBars,
    #[live] draw_block: DrawSquare,
    #[live] draw_name: DrawText,
    #[live] draw_time: DrawText,
    #[live] draw_reason: DrawText,
    #[rust] steps: Vec<crate::report::Stage>,
}
impl Widget for CiSteps {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if !self.scroll_bars.handle_event(cx, event, scope).is_empty() {self.area.redraw(cx);}
    }
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        self.scroll_bars.begin(cx, walk, Layout::flow_down());
        let viewport = cx.turtle().rect();
        let width = (viewport.size.x-8.0).max(0.0);
        if self.steps.is_empty() {
            self.draw_name.color = rgb(105, 115, 126);
            self.draw_name.draw_walk(cx, Walk::fill_fit(), Align::default(), "No steps recorded yet.");
        }
        for step in &self.steps {
            let row = cx.walk_turtle(Walk::fixed(width, 27.0));
            let failed = step.state == "failed";
            self.draw_name.color = if failed {colour("red")} else {rgb(125, 137, 149)};
            self.draw_name.draw_abs(cx, row.pos+dvec2(0.0, 3.0), match step.state.as_str(){"passed"=>"✓", "failed"=>"×", "running"=>"◦", "warning"=>"!", _=>"–"});
            self.draw_name.color = rgb(159, 168, 179);
            let name = step.name.rsplit(" / ").next().unwrap_or(&step.name);
            let label = ellipsis(&self.draw_name, cx, name, width-86.0);
            self.draw_name.draw_abs(cx, row.pos+dvec2(22.0, 3.0), &label);
            let seconds = step.started.filter(|_| step.state=="running").map(|s|s.elapsed().as_secs_f64()).unwrap_or(step.seconds);
            self.draw_time.draw_abs(cx, row.pos+dvec2(width-55.0, 4.0), &format!("{seconds:.1}s"));
            if matches!(step.state.as_str(), "failed" | "warning") && !step.detail.is_empty() {
                let text = self.draw_reason.layout(cx, 0.0, 0.0, Some((width-40.0).max(1.0) as f32), true, Align::default(), &step.detail);
                let height = text.size_in_lpxs.height as f64 + 20.0;
                let block = cx.walk_turtle(Walk::fixed(width, height+6.0));
                self.draw_block.draw_abs(cx, Rect{pos:block.pos+dvec2(20.0,0.0), size:dvec2(width-20.0,height)});
                self.draw_reason.draw_walk_laidout(cx, Walk::abs_rect(Rect{pos:block.pos+dvec2(30.0,10.0), size:dvec2(width-40.0,height-20.0)}), &text);
            }
        }
        self.scroll_bars.end(cx);
        cx.add_rect_area(&mut self.area, viewport);
        DrawStep::done()
    }
}
impl CiStepsRef {
    pub fn set_steps(&self, cx: &mut Cx, steps: Vec<crate::report::Stage>) {
        if let Some(mut inner) = self.borrow_mut() {inner.steps=steps; inner.area.redraw(cx);}
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
        assert!((5..=6).contains(&cols), "{cols} columns");
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
