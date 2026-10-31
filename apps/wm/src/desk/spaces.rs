//! What the desk draws of the spaces: the cached captures, the slide that
//! composites two of them, the strip of thumbnails and the cheat sheet.
//! The data and the gestures are in `crate::spaces`.
use super::*;
use crate::shell::{alpha, rgb, ui::{rect, HAlign}};
use crate::spaces::{app::strip_layout, sheet, strip::{Hit, Layout as StripLayout}};

/// `MAKEPAD_WM_MOTION=1` (see `draw_tile_inner`).
pub fn motion_trace() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("MAKEPAD_WM_MOTION").is_some())
}

/// `MAKEPAD_WM_TRACE_FRAMES=1` logs a line per desk frame: how often the
/// desk is drawn while something moves, and (proof of idle) that it is not
/// drawn while nothing does.
pub fn trace_frames() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("MAKEPAD_WM_TRACE_FRAMES").is_some())
}

/// A growing fullscreen window keeps the windows it covers under it this long.
pub const UNDERLAY_SECS: f64 = 0.6;

impl WmDesk {
    /// Draw `client`s of one space into a capture of their own, as they
    /// look now (a hidden space's windows keep their last frame).
    fn capture_space(
        &mut self,
        cx: &mut Cx2d,
        scope: &mut Scope,
        borders: &BorderTheme,
        key: (usize, usize),
        screen: Rect,
        list: &[(ClientId, LRect)],
        focus: Option<ClientId>,
    ) {
        for (c, r) in list {
            self.anims.entry(*c).or_insert_with(|| TileAnim::settled(*r, focus == Some(*c)));
        }
        // The shelf poses of this space's windows, from the animations just made.
        self.refresh_swell();
        let mut frame = self
            .space_shots
            .remove(&key)
            .unwrap_or_else(|| WindowFrame::new_with_name(cx, "wm_space_shot"));
        // The glass behind terminals samples the live stack: a capture has
        // no stack to sample.
        let composing = std::mem::replace(&mut self.composing, false);
        frame.begin(cx, screen);
        super::frametrace::set_cap(Some(key));
        for (c, _) in list {
            self.draw_tile(cx, scope, *c, borders);
        }
        super::frametrace::set_cap(None);
        super::frametrace::made(key, list.iter().map(|(c, _)| *c));
        frame.end(cx);
        self.composing = composing;
        self.space_shots.insert(key, frame);
        self.space_members.insert(key, (list.iter().map(|(c, _)| *c).collect(), false));
    }

    /// Make the captures the slides and the strip need, before the frame's
    /// animations are synced (so the space being left still has its tiles).
    /// `areas` are the displays' tiling areas.
    pub(super) fn capture_spaces(&mut self, cx: &mut Cx2d, scope: &mut Scope, borders: &BorderTheme, areas: &[(usize, LRect)]) {
        struct Job {
            key: (usize, usize),
            screen: Rect,
            list: Vec<(ClientId, LRect)>,
            focus: Option<ClientId>,
        }
        let mut jobs: Vec<Job> = Vec::new();
        let mut keep: Vec<(usize, usize)> = Vec::new();
        {
            let Some(state) = scope.data.get_mut::<WmState>() else { return };
            let gap = state.gap;
            let mut changed: Vec<(usize, usize)> = Vec::new();
            if state.spaces.stale {
                // Windows moved between spaces: the captures of the spaces
                // whose windows changed are old (a hidden space's windows do
                // not draw, so a capture of it is made once, when it is left,
                // and kept).
                state.spaces.stale = false;
                for (&key, (members, _)) in self.space_members.iter() {
                    let (i, slot) = key;
                    let layout = state.layout_at(i);
                    let Some(&(_, area)) = areas.iter().find(|(k, _)| *k == i) else { continue };
                    let now: Vec<ClientId> = layout.rects_of(slot, area, gap).into_iter().map(|(c, _)| c).collect();
                    let mut a = members.clone();
                    let mut b = now;
                    a.sort_unstable();
                    b.sort_unstable();
                    if a != b {
                        changed.push(key);
                    }
                }
            }
            {
                for &(i, area) in areas {
                    let layout = state.layout_at(i);
                    let n = layout.spaces.len();
                    let mut need: Vec<usize> = state.spaces.slides.get(i).map(|s| s.needed(n)).unwrap_or_default();
                    if state.spaces.strip.visible() && state.spaces.strip.screen == i {
                        need.extend(0..n);
                    }
                    // Every space that is not shown keeps a capture, for the
                    // slide to it.
                    if n > 1 {
                        need.extend((0..n).filter(|k| *k != layout.spaces.cur()));
                    }
                    need.sort_unstable();
                    need.dedup();
                    for idx in need {
                        let Some(sp) = layout.spaces.get(idx) else { continue };
                        let key = (i, sp.slot);
                        keep.push(key);
                        if self.space_shots.contains_key(&key) && !changed.contains(&key) {
                            continue;
                        }
                        let r = state.screens.screens[i].rect;
                        jobs.push(Job {
                            key,
                            screen: Rect { pos: dvec2(r.x, r.y), size: dvec2(r.w, r.h) },
                            list: layout.rects_of(sp.slot, area, gap),
                            focus: layout.workspaces[sp.slot].focus,
                        });
                    }
                }
            }
        }
        // Captures nobody is using any more are freed.
        self.drop_space_shots_where(cx, |k| !keep.contains(k));
        self.shots_made = !jobs.is_empty();
        for job in jobs {
            self.capture_space(cx, scope, borders, job.key, job.screen, &job.list, job.focus);
        }
    }

    fn drop_space_shots_where(&mut self, cx: &mut Cx2d, drop: impl Fn(&(usize, usize)) -> bool) {
        let gone: Vec<(usize, usize)> = self.space_shots.keys().filter(|k| drop(k)).copied().collect();
        for k in gone {
            self.space_members.remove(&k);
            if let Some(frame) = self.space_shots.remove(&k) {
                frame.forget(cx);
            }
        }
    }

    /// One capture (or a window onto it) drawn at `dst`.
    fn draw_shot(&mut self, cx: &mut Cx2d, key: (usize, usize), dst: Rect, uv: (f64, f64), radius: f32, opacity: f32) -> bool {
        let Some(frame) = self.space_shots.get_mut(&key) else { return false };
        frame.freeze(cx);
        // Once the capture holds its picture it stands on its own: the
        // windows' captures drawn into it are no longer its children, so a
        // window repainting cannot repaint (and blank) the space.
        if frame.frozen() {
            if let Some((members, done)) = self.space_members.get_mut(&key).filter(|(_, d)| !*d) {
                let mut all = true;
                for c in members.iter() {
                    if let Some(f) = self.desktop_frames.get_mut(c) {
                        f.freeze(cx);
                        all &= f.frozen();
                    }
                }
                *done = all;
            }
        }
        self.draw_phone.draw_vars.set_texture(0, frame.texture());
        self.draw_phone.uv_pos = vec2(uv.0 as f32, 0.0);
        self.draw_phone.uv_size = vec2((uv.1 - uv.0) as f32, 1.0);
        self.draw_phone.opacity = opacity;
        self.draw_phone.radius = radius * 0.5;
        self.draw_phone.draw_abs(cx, dst);
        true
    }

    /// The running slides: the two spaces in view, each shifted by its
    /// distance from the slide's position, cropped to the display.
    pub(super) fn draw_slides(&mut self, cx: &mut Cx2d, scope: &mut Scope) {
        let mut plan: Vec<((usize, usize), LRect, f64)> = Vec::new();
        if let Some(state) = scope.data.get_mut::<WmState>() {
            for i in 0..state.screens.live_count() {
                let Some(slide) = state.spaces.slides.get(i) else { continue };
                let layout = state.layout_at(i);
                let screen = state.screens.screens[i].rect;
                for (idx, off) in slide.views(layout.spaces.len()) {
                    if let Some(sp) = layout.spaces.get(idx) {
                        plan.push(((i, sp.slot), screen, off));
                    }
                }
            }
        }
        for (key, screen, off) in plan {
            let Some((dst, u0, u1)) = crate::spaces::slide::crop(screen, off) else { continue };
            let dst = Self::lrect_to_rect(dst);
            let drawn = self.draw_shot(cx, key, dst, (u0, u1), 0.0, 1.0);
            super::frametrace::shot(super::frametrace::Shot {
                key: (key.0 as u16, key.1 as u16),
                off: off as f32,
                dst: super::frametrace::r4(dst.pos.x, dst.pos.y, dst.size.x, dst.size.y),
                drawn,
            });
            if trace_frames() {
                log!("wm: slide shot {:?} off={:.3} dst={:?} uv=({:.3},{:.3}) drawn={}", key, off, dst, u0, u1, drawn);
            }
            if self.composing {
                if let Some(c) = self.compositor.as_mut() {
                    c.content(dst);
                }
            }
        }
    }

    /// The strip along the top of its display.
    pub(super) fn draw_strip(&mut self, cx: &mut Cx2d, scope: &mut Scope) {
        let Some(state) = scope.data.get_mut::<WmState>() else { return };
        let Some(layout) = strip_layout(state) else { return };
        let screen = state.spaces.strip.screen;
        let drag = state.spaces.strip.drag.filter(|d| d.moved);
        let hover = state.spaces.strip.hover;
        let accent = state.accent;
        let l = state.layout_at(screen);
        let cur = l.spaces.cur();
        let slots: Vec<(usize, Option<ClientId>)> = l.spaces.items().iter().map(|s| (s.slot, s.fullscreen_client())).collect();
        let counts: Vec<usize> = slots.iter().map(|(slot, _)| l.clients_on(*slot).len()).collect();
        let t = state.spaces.strip.t() as f32;
        self.draw_strip_layout(cx, screen, &layout, &slots, &counts, cur, hover, drag, accent, t);
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_strip_layout(
        &mut self,
        cx: &mut Cx2d,
        screen: usize,
        layout: &StripLayout,
        slots: &[(usize, Option<ClientId>)],
        counts: &[usize],
        cur: usize,
        hover: Option<Hit>,
        drag: Option<crate::spaces::strip::Drag>,
        accent: Vec4f,
        t: f32,
    ) {
        let dim = |a: f32| alpha(rgb(14, 15, 20), a * t);
        let ink = alpha(rgb(235, 238, 245), t);
        let soft = alpha(rgb(235, 238, 245), 0.6 * t);
        let band = Self::lrect_to_rect(layout.band);
        self.shell_draw.solid(cx, band, dim(0.86));
        self.shell_draw.solid(cx, rect(band.pos.x, band.pos.y + band.size.y - 1.0, band.size.x, 1.0), alpha(rgb(255, 255, 255), 0.12 * t));
        for (cell, (slot, fs)) in layout.cells.iter().zip(slots) {
            let thumb = Self::lrect_to_rect(cell.thumb);
            let moved = drag.filter(|d| d.index == cell.index);
            let (thumb, lift) = match moved {
                Some(d) => (Rect { pos: thumb.pos + dvec2(d.at.0 - d.start.0, d.at.1 - d.start.1), size: thumb.size }, 0.8),
                None => (thumb, 1.0),
            };
            self.shell_draw.solid(cx, rect(thumb.pos.x - 1.0, thumb.pos.y - 1.0, thumb.size.x + 2.0, thumb.size.y + 2.0), alpha(rgb(255, 255, 255), 0.10 * t));
            self.shell_draw.solid(cx, thumb, alpha(rgb(30, 32, 40), 0.9 * t));
            self.draw_shot(cx, (screen, *slot), thumb, (0.0, 1.0), 6.0, t * lift);
            let hot = cell.index == cur || hover == Some(Hit::Thumb(cell.index));
            let ring = if cell.index == cur { accent } else { alpha(rgb(255, 255, 255), 0.5) };
            if hot {
                let w = 2.0;
                let c = alpha(ring, t);
                for r in [
                    rect(thumb.pos.x - w, thumb.pos.y - w, thumb.size.x + 2.0 * w, w),
                    rect(thumb.pos.x - w, thumb.pos.y + thumb.size.y, thumb.size.x + 2.0 * w, w),
                    rect(thumb.pos.x - w, thumb.pos.y, w, thumb.size.y),
                    rect(thumb.pos.x + thumb.size.x, thumb.pos.y, w, thumb.size.y),
                ] {
                    self.shell_draw.solid(cx, r, c);
                }
            }
            let label = match fs {
                Some(c) => format!("Fullscreen: {}", self.titles.get(c).cloned().unwrap_or_default()),
                None => {
                    let n = slots[..=cell.index].iter().filter(|(_, f)| f.is_none()).count();
                    let w = counts.get(cell.index).copied().unwrap_or(0);
                    if w == 0 { format!("Desktop {n}") } else { format!("Desktop {n}  ·  {w}") }
                }
            };
            let lab = Self::lrect_to_rect(cell.label);
            self.shell_draw.label_elided(cx, lab, false, 12.0, if cell.index == cur { ink } else { soft }, HAlign::Center, &label);
            if let Some(x) = cell.close {
                let x = Self::lrect_to_rect(x);
                let on = hover == Some(Hit::Close(cell.index));
                self.shell_draw.solid(cx, x, alpha(if on { rgb(224, 74, 66) } else { rgb(20, 22, 28) }, 0.85 * t));
                self.shell_draw.label(cx, x, true, 13.0, ink, HAlign::Center, "x");
            }
        }
        if let Some(p) = layout.plus {
            let p = Self::lrect_to_rect(p);
            let on = hover == Some(Hit::Plus);
            self.shell_draw.solid(cx, p, alpha(rgb(255, 255, 255), if on { 0.14 } else { 0.07 } * t));
            self.shell_draw.label(cx, p, false, 40.0, soft, HAlign::Center, "+");
        }
    }

    /// The cheat sheet: every binding, in columns, over a dimmed desk.
    pub(super) fn draw_sheet(&mut self, cx: &mut Cx2d, scope: &mut Scope, desk: Rect) {
        let open = scope.data.get_mut::<WmState>().is_some_and(|s| s.spaces.sheet);
        if !open {
            return;
        }
        let accent = scope.data.get_mut::<WmState>().map_or(rgb(120, 160, 255), |s| s.accent);
        self.shell_draw.solid(cx, desk, alpha(rgb(8, 9, 12), 0.72));
        let card = rect(desk.pos.x + desk.size.x * 0.04, desk.pos.y + desk.size.y * 0.06, desk.size.x * 0.92, desk.size.y * 0.88);
        self.shell_draw.solid(cx, card, alpha(rgb(22, 24, 30), 0.96));
        self.shell_draw.label(cx, rect(card.pos.x + 24.0, card.pos.y + 14.0, card.size.x - 48.0, 30.0), true, 18.0, rgb(240, 242, 248), HAlign::Left, "Keyboard shortcuts   (Esc or Super+/ closes)");
        // Sections are blocks that never split: 2 to 4 columns by width, each
        // block goes under the shortest column so far.
        let secs = sheet::sections();
        let top = card.pos.y + 60.0;
        let line_h = 20.0;
        let cols = ((card.size.x / 520.0).floor() as usize).clamp(2, 4);
        let col_w = (card.size.x - 48.0) / cols as f64;
        let mut heights = vec![0usize; cols];
        for sec in &secs {
            let c = (0..cols).min_by_key(|c| heights[*c]).unwrap_or(0);
            let x = card.pos.x + 24.0 + c as f64 * col_w;
            let y = top + heights[c] as f64 * line_h;
            self.shell_draw.label(cx, rect(x, y, col_w - 12.0, line_h), true, 13.0, accent, HAlign::Left, sec.title);
            for (k, r) in sec.rows.iter().enumerate() {
                let y = top + (heights[c] + 1 + k) as f64 * line_h;
                let combo = r.combo.split("  (").next().unwrap_or(&r.combo);
                self.shell_draw.label_elided(cx, rect(x, y, col_w * 0.42, line_h), false, 12.0, rgb(240, 242, 248), HAlign::Left, combo);
                self.shell_draw.label_elided(cx, rect(x + col_w * 0.42, y, col_w * 0.58 - 14.0, line_h), false, 12.0, alpha(rgb(220, 224, 234), 0.72), HAlign::Left, &r.help);
            }
            heights[c] += sec.rows.len() + 2;
        }
    }
}
