use crate::report::ScriptState;
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    mod.widgets.CiWall = #(CiWall::register_widget(vm)){
        width: Fill height: Fill
        draw_square: #(DrawSquare::script_shader(vm)){
            color: instance(#3a3f46)
            pulse: instance(1.0)
            progress: instance(-1.0)
            selected: instance(0.0)
            pixel: fn(){
                let p = self.pos * self.rect_size
                let edge = min(min(p.x, p.y), min(self.rect_size.x-p.x, self.rect_size.y-p.y))
                if edge < 3.0 && self.selected > 0.5 {return #fff}
                if self.progress >= 0.0 && p.y > self.rect_size.y-8.0 {
                    if self.pos.x < self.progress {return #fff}
                    return #0008
                }
                return vec4(self.color.rgb * self.pulse, 1.0)
            }
        }
        draw_text +: {color: #fff text_style +: {font_size: 20}}
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
    draw_text: DrawText,
    #[rust]
    tiles: Vec<Tile>,
    #[rust]
    rects: Vec<Rect>,
    #[rust]
    selected: Option<String>,
    #[rust]
    next_frame: NextFrame,
    #[rust]
    time: f64,
}
pub fn grid(width: f64, height: f64, count: usize) -> (usize, f64) {
    if count == 0 || width <= 0.0 || height <= 0.0 {
        return (1, 0.0);
    }
    let mut best = (1, 0.0);
    for cols in 1..=count {
        let rows = count.div_ceil(cols);
        let side = (width / cols as f64).min(height / rows as f64);
        if side > best.1 {
            best = (cols, side);
        }
    }
    best
}
impl Widget for CiWall {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if let Hit::FingerDown(e) = event.hits(cx, self.area) {
            if let Some(index) = self.rects.iter().position(|r| r.contains(e.abs)) {
                self.selected = Some(self.tiles[index].key.clone());
                self.area.redraw(cx);
            }
        }
        if let Event::NextFrame(e) = event {
            if e.set.contains(&self.next_frame)
                && self.tiles.iter().any(|t| t.state.verdict == "running")
            {
                self.time = e.time;
                self.area.redraw(cx);
                self.next_frame = cx.new_next_frame();
            }
        }
    }
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let rect = cx.walk_turtle(walk);
        cx.add_rect_area(&mut self.area, rect);
        let (cols, cell) = grid(rect.size.x, rect.size.y, self.tiles.len());
        self.rects.clear();
        self.draw_square.begin_many_instances(cx);
        for (index, tile) in self.tiles.iter().enumerate() {
            let gap = (cell * 0.035).min(10.0);
            let side = (cell - gap).max(0.0);
            let r = Rect {
                pos: rect.pos + dvec2((index % cols) as f64 * cell, (index / cols) as f64 * cell),
                size: dvec2(side, side),
            };
            self.rects.push(r);
            self.draw_square.color = match tile.state.color_verdict() {
                "green" => vec4(32. / 255., 160. / 255., 64. / 255., 1.),
                "orange" => vec4(240. / 255., 140. / 255., 30. / 255., 1.),
                "red" => vec4(1., 32. / 255., 32. / 255., 1.),
                _ => vec4(58. / 255., 63. / 255., 70. / 255., 1.),
            };
            let running = tile.state.verdict == "running";
            self.draw_square.pulse = if running {
                0.88 + 0.12 * (self.time * 2.5).sin() as f32
            } else {
                1.0
            };
            let (p, w, f) = tile.state.counts();
            self.draw_square.progress = if running {
                ((p + w + f) as f32 / (tile.state.steps.len() + 1) as f32).min(0.98)
            } else {
                -1.0
            };
            self.draw_square.selected = if self.selected.as_ref() == Some(&tile.key) {
                1.0
            } else {
                0.0
            };
            self.draw_square.draw_abs(cx, r);
        }
        self.draw_square.end_many_instances(cx);
        self.draw_text.begin_many_instances(cx);
        for (tile, r) in self.tiles.iter().zip(&self.rects) {
            let font = (r.size.x / 11.0).clamp(9.0, 30.0) as f32;
            self.draw_text.text_style.font_size = font;
            let chars = ((r.size.x - 20.0) / (font as f64 * 0.62)).max(1.0) as usize;
            let (p, w, f) = tile.state.counts();
            let seconds = if tile.state.verdict == "running" {
                tile.state
                    .started
                    .map(|t| t.elapsed().as_secs_f64())
                    .unwrap_or(tile.state.seconds)
            } else {
                tile.state.seconds
            };
            let step = tile
                .state
                .steps
                .iter()
                .find(|s| s.state == "failed")
                .or_else(|| tile.state.steps.iter().rev().find(|s| s.state == "running"))
                .or_else(|| tile.state.steps.last())
                .map(|s| s.name.rsplit(" / ").next().unwrap_or(&s.name))
                .unwrap_or("waiting");
            let lines = [
                tile.label.clone(),
                step.into(),
                format!("{p} passed / {w} warn / {f} fail"),
                format!("{seconds:.0}s"),
                tile.state.detail.lines().next().unwrap_or("").into(),
            ];
            for (index, line) in lines.iter().enumerate() {
                let y = 10.0 + index as f64 * (font as f64 * 1.6);
                if y + font as f64 > r.size.y - 12.0 {
                    break;
                }
                let text = if line.chars().count() > chars {
                    format!(
                        "{}…",
                        line.chars()
                            .take(chars.saturating_sub(1))
                            .collect::<String>()
                    )
                } else {
                    line.clone()
                };
                self.draw_text.draw_abs(cx, r.pos + dvec2(10.0, y), &text);
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
            if running && !was_running {
                inner.next_frame = cx.new_next_frame();
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
    fn squares_fit_and_are_maximal() {
        for count in [1, 2, 3, 8, 50, 101] {
            let (cols, size) = grid(1000.0, 700.0, count);
            assert!(cols as f64 * size <= 1000.001);
            assert!(count.div_ceil(cols) as f64 * size <= 700.001);
            for alt in 1..=count {
                assert!(
                    size + 0.001 >= (1000.0 / alt as f64).min(700.0 / count.div_ceil(alt) as f64)
                );
            }
        }
    }
}
