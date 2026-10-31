//! The studio's lists, drawn directly: the preset library (group headers
//! and cards: a tile, a title, what goes in and where it lands, a
//! readiness badge), the run history, the results, and — in tile mode — the
//! library picker for media inputs. One widget; the parent supplies rows
//! and reads [`CardsAction`].

use makepad_widgets::*;

const HEADER_H: f64 = 24.0;
const CARD_H: f64 = 50.0;
const TILE: f64 = 36.0;
const GRID_MIN: f64 = 92.0;
const GAP: f64 = 6.0;
const GRID_CAPTION: f64 = 28.0;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    let stage = mod.stage

    set_type_default() do #(DrawStudioPicture::script_shader(vm)){
        ..mod.draw.DrawQuad
        image: texture_2d(float)
        pixel: fn() {
            let c = self.image.sample(self.pos)
            return vec4(c.xyz * self.opacity, self.opacity)
        }
    }

    set_type_default() do #(DrawStudioTile::script_shader(vm)){
        ..mod.draw.DrawQuad
        pixel: fn() {
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            sdf.box(0.5, 0.5, self.rect_size.x - 1.0, self.rect_size.y - 1.0, self.radius)
            sdf.fill_keep(self.fill)
            sdf.stroke(self.edge, self.edge_width)
            return sdf.result
        }
    }

    mod.widgets.StudioCardsBase = #(StudioCards::register_widget(vm))
    mod.widgets.StudioCards = set_type_default() do mod.widgets.StudioCardsBase{
        width: Fill height: Fill
        draw_bg +: {color: stage.panel}
        draw_title +: {color: stage.text_hi text_style: theme.font_bold{font_size: 9}}
        draw_detail +: {color: stage.label text_style: theme.font_regular{font_size: 8}}
        draw_head +: {color: stage.secondary text_style: theme.font_bold{font_size: 8}}
        draw_glyph +: {color: stage.text_max text_style: theme.font_bold{font_size: 8}}
        draw_badge +: {text_style: theme.font_bold{font_size: 7}}
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawStudioPicture {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    pub opacity: f32,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawStudioTile {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    pub fill: Vec4f,
    #[live]
    pub edge: Vec4f,
    #[live]
    pub edge_width: f32,
    #[live]
    pub radius: f32,
}

/// One row: a group header, or a card with a glyph tile or a picture.
#[derive(Clone, Debug, Default)]
pub struct CardRow {
    pub key: String,
    pub header: bool,
    pub title: String,
    pub detail: String,
    pub glyph: String,
    /// `0xRRGGBBAA` of the tile (and of a header's rule).
    pub tint: u32,
    pub badge: String,
    pub badge_tint: u32,
    pub texture: Option<Texture>,
    /// Drawn dimmer: a card the parent marks as not ready to act on.
    pub quiet: bool,
}

#[derive(Clone, Debug, Default)]
pub enum CardsAction {
    #[default]
    None,
    Select(String),
    /// A second click on the selected card.
    Activate(String),
}

#[derive(Script, ScriptHook, Widget)]
pub struct StudioCards {
    #[uid]
    uid: WidgetUid,
    #[walk]
    walk: Walk,
    #[redraw]
    #[live]
    draw_bg: DrawColor,
    #[live]
    draw_tile: DrawStudioTile,
    #[live]
    draw_picture: DrawStudioPicture,
    #[live]
    draw_title: DrawText,
    #[live]
    draw_detail: DrawText,
    #[live]
    draw_head: DrawText,
    #[live]
    draw_glyph: DrawText,
    #[live]
    draw_badge: DrawText,
    /// A grid of pictures with captions instead of card rows.
    #[live]
    tiles: bool,
    #[rust]
    rows: Vec<CardRow>,
    #[rust]
    selected: Option<String>,
    #[rust]
    hover: Option<usize>,
    #[rust]
    scroll: f64,
    #[rust]
    rect: Rect,
    #[rust]
    empty: String,
}

impl StudioCards {
    pub fn set_rows(&mut self, cx: &mut Cx, rows: Vec<CardRow>, empty: &str) {
        self.rows = rows;
        self.empty = empty.to_string();
        self.hover = None;
        self.clamp_scroll();
        self.redraw(cx);
    }

    pub fn set_selected(&mut self, cx: &mut Cx, key: Option<&str>) {
        let next = key.map(str::to_string);
        if self.selected != next {
            self.selected = next;
            self.redraw(cx);
        }
    }

    pub fn reset_scroll(&mut self, cx: &mut Cx) {
        self.scroll = 0.0;
        self.redraw(cx);
    }

    fn grid(&self) -> (usize, f64, f64) {
        let inner = (self.rect.size.x - GAP).max(GRID_MIN);
        let columns = ((inner / (GRID_MIN + GAP)).floor() as usize).max(1);
        let w = inner / columns as f64 - GAP;
        (columns, w, w * 0.75 + GRID_CAPTION)
    }

    /// Row rectangles in content space (before scrolling).
    fn row_rect(&self, index: usize) -> Rect {
        if self.tiles {
            let (columns, w, h) = self.grid();
            let (col, row) = (index % columns, index / columns);
            return Rect { pos: dvec2(GAP + col as f64 * (w + GAP), GAP + row as f64 * (h + GAP)), size: dvec2(w, h) };
        }
        let mut y = 0.0;
        for row in &self.rows[..index.min(self.rows.len())] {
            y += if row.header { HEADER_H } else { CARD_H };
        }
        let h = if self.rows.get(index).is_some_and(|row| row.header) { HEADER_H } else { CARD_H };
        Rect { pos: dvec2(0.0, y), size: dvec2(self.rect.size.x, h) }
    }

    fn content_height(&self) -> f64 {
        if self.rows.is_empty() {
            return 0.0;
        }
        let last = self.row_rect(self.rows.len() - 1);
        last.pos.y + last.size.y + GAP
    }

    fn clamp_scroll(&mut self) {
        let max = (self.content_height() - self.rect.size.y).max(0.0);
        self.scroll = self.scroll.clamp(0.0, max);
    }

    /// The row under `abs`. `area` is the hit's own rectangle in window
    /// space: inside a scrolled or shifted container the drawn rectangle is
    /// in that container's coordinates, the event's in the window's.
    fn row_at(&self, area: Rect, abs: Vec2d) -> Option<usize> {
        if !area.contains(abs) {
            return None;
        }
        let local = dvec2(abs.x - area.pos.x, abs.y - area.pos.y + self.scroll);
        (0..self.rows.len()).find(|index| !self.rows[*index].header && self.row_rect(*index).contains(local))
    }
}

impl Widget for StudioCards {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        let uid = self.widget_uid();
        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerHoverIn(fe) | Hit::FingerHoverOver(fe) => {
                let hover = self.row_at(fe.rect, fe.abs);
                if hover != self.hover {
                    self.hover = hover;
                    self.redraw(cx);
                }
                cx.set_cursor(if hover.is_some() { MouseCursor::Hand } else { MouseCursor::Default });
            }
            Hit::FingerHoverOut(_) => {
                if self.hover.take().is_some() {
                    self.redraw(cx);
                }
            }
            Hit::FingerUp(fe) if fe.is_over && !fe.cancelled => {
                if let Some(index) = self.row_at(fe.rect, fe.abs) {
                    let key = self.rows[index].key.clone();
                    if self.selected.as_deref() == Some(key.as_str()) {
                        cx.widget_action(uid, CardsAction::Activate(key));
                    } else {
                        self.selected = Some(key.clone());
                        cx.widget_action(uid, CardsAction::Select(key));
                    }
                    self.redraw(cx);
                }
            }
            Hit::FingerScroll(fe) => {
                self.scroll += fe.scroll.y;
                self.clamp_scroll();
                self.redraw(cx);
            }
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let rect = cx.walk_turtle(walk);
        self.rect = rect;
        self.clamp_scroll();
        self.draw_bg.draw_abs(cx, rect);
        if self.rows.is_empty() {
            let empty = self.empty.clone();
            self.draw_detail.draw_abs(cx, dvec2(rect.pos.x + 10.0, rect.pos.y + 10.0), &empty);
            return DrawStep::done();
        }
        let p = makepad_look::palette::get(cx);
        let clip_top = rect.pos.y;
        let clip_bottom = rect.pos.y + rect.size.y;
        for index in 0..self.rows.len() {
            let local = self.row_rect(index);
            let r = Rect { pos: dvec2(rect.pos.x + local.pos.x, rect.pos.y + local.pos.y - self.scroll), size: local.size };
            // Whole rows only, so no text is ever cut by the panel edge.
            if r.pos.y < clip_top - 0.5 {
                continue;
            }
            if r.pos.y + r.size.y > clip_bottom + 0.5 {
                break;
            }
            let row = self.rows[index].clone();
            let selected = self.selected.as_deref() == Some(row.key.as_str()) && !row.header;
            let hover = self.hover == Some(index);
            if self.tiles {
                self.draw_grid_tile(cx, &p, r, &row, selected, hover);
            } else if row.header {
                self.draw_head.draw_abs(cx, dvec2(r.pos.x + 10.0, r.pos.y + 8.0), &row.title);
                self.draw_tile.fill = Vec4f::from_u32(p.separator);
                self.draw_tile.edge = Vec4f::from_u32(0);
                self.draw_tile.edge_width = 0.0;
                self.draw_tile.radius = 0.0;
                self.draw_tile.draw_abs(cx, Rect { pos: dvec2(r.pos.x + 10.0, r.pos.y + r.size.y - 2.0), size: dvec2(r.size.x - 20.0, 1.0) });
            } else {
                self.draw_card(cx, &p, r, &row, selected, hover);
            }
        }
        DrawStep::done()
    }
}

impl StudioCards {
    fn clip(text: &str, width: f64, per_char: f64) -> String {
        let chars = ((width / per_char).max(3.0)) as usize;
        if text.chars().count() > chars {
            format!("{}…", text.chars().take(chars.saturating_sub(1)).collect::<String>())
        } else {
            text.to_string()
        }
    }

    fn draw_card(&mut self, cx: &mut Cx2d, p: &makepad_look::palette::Palette, r: Rect, row: &CardRow, selected: bool, hover: bool) {
        let card = Rect { pos: dvec2(r.pos.x + 6.0, r.pos.y + 2.0), size: dvec2(r.size.x - 12.0, r.size.y - 4.0) };
        self.draw_tile.fill = Vec4f::from_u32(if selected { p.row_picked } else if hover { p.control_hover } else { p.row });
        self.draw_tile.edge = Vec4f::from_u32(if selected { p.accent } else { p.separator });
        self.draw_tile.edge_width = 1.0;
        self.draw_tile.radius = 4.0;
        self.draw_tile.draw_abs(cx, card);
        let tile = Rect { pos: dvec2(card.pos.x + 6.0, card.pos.y + (card.size.y - TILE) * 0.5), size: dvec2(TILE, TILE) };
        match &row.texture {
            Some(texture) => {
                self.draw_picture.draw_vars.set_texture(0, texture);
                self.draw_picture.opacity = if row.quiet { 0.55 } else { 1.0 };
                self.draw_picture.draw_abs(cx, tile);
            }
            None => {
                let mut tint = Vec4f::from_u32(row.tint);
                tint.w = if row.quiet { 0.35 } else { 0.85 };
                self.draw_tile.fill = tint;
                self.draw_tile.edge = Vec4f::from_u32(0);
                self.draw_tile.edge_width = 0.0;
                self.draw_tile.radius = 3.0;
                self.draw_tile.draw_abs(cx, tile);
                let glyph_w = row.glyph.chars().count() as f64 * 6.0;
                self.draw_glyph.draw_abs(cx, dvec2(tile.pos.x + (TILE - glyph_w) * 0.5, tile.pos.y + TILE * 0.5 - 6.0), &row.glyph);
            }
        }
        let text_x = tile.pos.x + TILE + 9.0;
        let badge_w = if row.badge.is_empty() { 0.0 } else { row.badge.chars().count() as f64 * 6.0 + 12.0 };
        let text_w = card.pos.x + card.size.x - text_x - badge_w - 8.0;
        self.draw_title.color = Vec4f::from_u32(if row.quiet { p.secondary } else { p.text_hi });
        self.draw_title.draw_abs(cx, dvec2(text_x, card.pos.y + 8.0), &Self::clip(&row.title, text_w, 6.4));
        self.draw_detail.draw_abs(cx, dvec2(text_x, card.pos.y + 25.0), &Self::clip(&row.detail, text_w, 5.2));
        if !row.badge.is_empty() {
            let badge = Rect { pos: dvec2(card.pos.x + card.size.x - badge_w - 6.0, card.pos.y + 7.0), size: dvec2(badge_w, 15.0) };
            let mut wash = Vec4f::from_u32(row.badge_tint);
            wash.w = 0.16;
            self.draw_tile.fill = wash;
            self.draw_tile.edge = Vec4f::from_u32(row.badge_tint);
            self.draw_tile.edge_width = 1.0;
            self.draw_tile.radius = 7.0;
            self.draw_tile.draw_abs(cx, badge);
            self.draw_badge.color = Vec4f::from_u32(row.badge_tint);
            self.draw_badge.draw_abs(cx, dvec2(badge.pos.x + 6.0, badge.pos.y + 3.0), &row.badge);
        }
    }

    fn draw_grid_tile(&mut self, cx: &mut Cx2d, p: &makepad_look::palette::Palette, r: Rect, row: &CardRow, selected: bool, hover: bool) {
        let thumb = Rect { pos: r.pos, size: dvec2(r.size.x, r.size.y - GRID_CAPTION) };
        self.draw_tile.fill = Vec4f::from_u32(if hover { p.control_hover } else { p.well });
        self.draw_tile.edge = Vec4f::from_u32(if selected { p.accent } else { p.border });
        self.draw_tile.edge_width = if selected { 2.0 } else { 1.0 };
        self.draw_tile.radius = 3.0;
        self.draw_tile.draw_abs(cx, thumb);
        let inner = Rect { pos: dvec2(thumb.pos.x + 2.0, thumb.pos.y + 2.0), size: dvec2(thumb.size.x - 4.0, thumb.size.y - 4.0) };
        match &row.texture {
            Some(texture) => {
                let (tw, th) = texture.get_format(cx).vec_width_height().unwrap_or((4, 3));
                let aspect = tw.max(1) as f64 / th.max(1) as f64;
                let (w, h) = if aspect > inner.size.x / inner.size.y { (inner.size.x, inner.size.x / aspect) } else { (inner.size.y * aspect, inner.size.y) };
                self.draw_picture.draw_vars.set_texture(0, texture);
                self.draw_picture.opacity = 1.0;
                self.draw_picture.draw_abs(cx, Rect { pos: dvec2(inner.pos.x + (inner.size.x - w) * 0.5, inner.pos.y + (inner.size.y - h) * 0.5), size: dvec2(w, h) });
            }
            None => {
                let mut tint = Vec4f::from_u32(row.tint);
                tint.w = 0.4;
                self.draw_tile.fill = tint;
                self.draw_tile.edge = Vec4f::from_u32(0);
                self.draw_tile.edge_width = 0.0;
                self.draw_tile.draw_abs(cx, inner);
            }
        }
        self.draw_title.color = Vec4f::from_u32(p.text_hi);
        self.draw_title.draw_abs(cx, dvec2(r.pos.x + 1.0, thumb.pos.y + thumb.size.y + 3.0), &Self::clip(&row.title, r.size.x, 6.4));
        self.draw_detail.draw_abs(cx, dvec2(r.pos.x + 1.0, thumb.pos.y + thumb.size.y + 16.0), &Self::clip(&row.detail, r.size.x, 5.2));
    }
}

impl StudioCardsRef {
    pub fn set_rows(&self, cx: &mut Cx, rows: Vec<CardRow>, empty: &str) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_rows(cx, rows, empty);
        }
    }

    pub fn set_selected(&self, cx: &mut Cx, key: Option<&str>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_selected(cx, key);
        }
    }

    pub fn reset_scroll(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.reset_scroll(cx);
        }
    }

    pub fn actions(&self, actions: &Actions) -> Vec<CardsAction> {
        actions.filter_widget_actions_cast::<CardsAction>(self.widget_uid()).collect()
    }
}
