//! A panel: the picture on a card. The app supplies rows of cells (buttons, toggles, text);
//! the panel lays them out in texels, hit-tests them, shows the hovered one and redraws its
//! texture only when what it shows changed. Colours and text come from the system theme.

use crate::card::Card;
use makepad_widgets::*;
use std::fmt::{Display, Write as _};
use std::ops::Range;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.math.*
    use mod.shader.*

    // A card in the room: its front shows the panel's texture, its edge and back the card's
    // colour.
    mod.draw.DrawXrCard = mod.std.set_type_default() do #(DrawXrCard::script_shader(vm)){
        ..mod.draw.DrawCube
        image: texture_2d(float)
        v_uv: varying(vec2f)
        v_front: varying(float)
        vertex: fn() {
            let pos = self.get_size() * self.geom.geom_pos + self.get_pos()
            let model_view = self.draw_list.view_transform * self.transform
            self.world = model_view * vec4(pos.x, pos.y, pos.z, 1.0)
            self.v_uv = self.geom.geom_uv
            self.v_front = step(0.5, self.geom.geom_normal.z)
            self.lit_color = self.color
            self.vertex_pos = self.draw_pass.camera_projection * (self.draw_pass.camera_view * self.world)
        }
        pixel: fn() {
            if self.v_front < 0.5 {
                return vec4(self.color.xyz * 0.6, 1.0)
            }
            let c = self.image.sample(self.v_uv)
            return vec4(c.xyz + self.color.xyz * (1.0 - c.w), 1.0)
        }
    }

    mod.widgets.XrPanel = set_type_default() do #(XrPanel::script_component(vm)){
        draw_card: mod.draw.DrawXrCard{color: theme.color_bg_app}
        draw_bg +: {color: theme.color_bg_app}
        draw_text +: {color: theme.color_text text_style: theme.font_regular{font_size: 22.0}}
        color_bg: theme.color_bg_app
        color_cell: theme.color_outset
        color_cell_hover: theme.color_outset_hover
        color_on: theme.color_focus
        color_text: theme.color_text
        color_meta: theme.color_text_meta
        color_error: theme.color_error
        color_warning: theme.color_warning
        color_info: theme.color_info
        color_good: theme.color_success
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawXrCard {
    #[deref]
    pub draw_super: DrawCube,
}

/// What a text says about itself: its colour.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Tone {
    #[default]
    Normal,
    Meta,
    Error,
    Warning,
    Info,
    Good,
}

/// A cell's width in its row: a share of what fixed cells leave, or texels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Width {
    Flex(f32),
    Fixed(f64),
}

impl Width {
    pub const FILL: Width = Width::Flex(1.0);
}

/// A row's height: texels, or a share of what fixed rows leave (a list).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Height {
    Fixed(f64),
    Fill,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CellKind {
    /// A pressable cell.
    Button,
    /// A pressable cell that shows a state (on: the accent).
    Toggle(bool),
    /// Text, left aligned, not pressable.
    Text,
    /// Text centred in its cell, not pressable.
    Heading,
    /// An empty slot that keeps its place and its id (an unused list row): not drawn, hit.
    Blank,
}

impl CellKind {
    fn pressable(self) -> bool {
        matches!(self, CellKind::Button | CellKind::Toggle(_) | CellKind::Blank)
    }
}

#[derive(Clone, Debug)]
struct Cell {
    id: u32,
    kind: CellKind,
    width: Width,
    tone: Tone,
    text: Range<usize>,
    rect: Rect,
}

#[derive(Clone, Debug)]
struct Row {
    height: Height,
    cells: Range<usize>,
}

/// The rows the app writes into the panel ([`XrPanel::set`]).
#[derive(Default)]
pub struct Rows {
    rows: Vec<Row>,
    cells: Vec<Cell>,
    text: String,
}

impl Rows {
    fn clear(&mut self) {
        self.rows.clear();
        self.cells.clear();
        self.text.clear();
    }

    /// A new row of `height`; the cells that follow go in it.
    pub fn row(&mut self, height: Height) -> &mut Self {
        let at = self.cells.len();
        self.rows.push(Row { height, cells: at..at });
        self
    }

    fn cell(&mut self, id: u32, kind: CellKind, width: Width, tone: Tone, label: impl Display) -> &mut Self {
        if self.rows.is_empty() {
            self.row(Height::Fixed(52.0));
        }
        let from = self.text.len();
        let _ = write!(self.text, "{label}");
        self.cells.push(Cell { id, kind, width, tone, text: from..self.text.len(), rect: Rect::default() });
        self.rows.last_mut().unwrap().cells.end = self.cells.len();
        self
    }

    pub fn button(&mut self, id: u32, label: impl Display, width: Width) -> &mut Self {
        self.cell(id, CellKind::Button, width, Tone::Normal, label)
    }

    pub fn toggle(&mut self, id: u32, label: impl Display, on: bool, width: Width) -> &mut Self {
        self.cell(id, CellKind::Toggle(on), width, Tone::Normal, label)
    }

    /// A pressable row of text in a tone (a list entry).
    pub fn entry(&mut self, id: u32, label: impl Display, tone: Tone, width: Width) -> &mut Self {
        self.cell(id, CellKind::Button, width, tone, label)
    }

    pub fn text(&mut self, label: impl Display, tone: Tone, width: Width) -> &mut Self {
        self.cell(u32::MAX, CellKind::Text, width, tone, label)
    }

    pub fn heading(&mut self, label: impl Display, tone: Tone, width: Width) -> &mut Self {
        self.cell(u32::MAX, CellKind::Heading, width, tone, label)
    }

    pub fn blank(&mut self, id: u32, width: Width) -> &mut Self {
        self.cell(id, CellKind::Blank, width, Tone::Normal, "")
    }
}

#[derive(Script, ScriptHook)]
pub struct XrPanel {
    /// The texture's size (texels).
    #[live(512.0)]
    pub width: f64,
    #[live(384.0)]
    pub height: f64,
    /// The card's width in the room (m); its height follows the texture's aspect.
    #[live(0.16)]
    pub meters: f32,
    /// The margin and the space between cells (texels).
    #[live(12.0)]
    pub pad: f64,
    /// The space between list rows (texels).
    #[live(4.0)]
    pub list_gap: f64,
    #[live]
    draw_card: DrawXrCard,
    #[live]
    draw_bg: DrawColor,
    #[live]
    draw_text: DrawText,
    #[live]
    color_bg: Vec4f,
    #[live]
    color_cell: Vec4f,
    #[live]
    color_cell_hover: Vec4f,
    #[live]
    color_on: Vec4f,
    #[live]
    color_text: Vec4f,
    #[live]
    color_meta: Vec4f,
    #[live]
    color_error: Vec4f,
    #[live]
    color_warning: Vec4f,
    #[live]
    color_info: Vec4f,
    #[live]
    color_good: Vec4f,
    #[rust]
    rows: Rows,
    /// What the rows show, as the app's key (None: never set).
    #[rust]
    key: Option<u64>,
    #[rust]
    hover: Option<u32>,
    #[rust]
    dirty: bool,
    #[rust]
    tex: Option<PanelTexture>,
}

struct PanelTexture {
    pass: DrawPass,
    list: DrawList,
    texture: Texture,
}

impl XrPanel {
    /// The card's size in the room (m).
    pub fn size(&self) -> Vec2f {
        vec2f(self.meters, self.meters * (self.height / self.width.max(1.0)) as f32)
    }

    /// The rows: `fill` writes them when `key` (what they show) changed since the last call.
    pub fn set(&mut self, key: u64, fill: impl FnOnce(&mut Rows)) {
        if self.key == Some(key) {
            return;
        }
        self.key = Some(key);
        self.rows.clear();
        fill(&mut self.rows);
        self.layout();
        self.dirty = true;
    }

    /// The pressable cell under the hover (shown lit).
    pub fn set_hover(&mut self, hover: Option<u32>) {
        if self.hover != hover {
            self.hover = hover;
            self.dirty = true;
        }
    }

    pub fn hover(&self) -> Option<u32> {
        self.hover
    }

    fn layout(&mut self) {
        let (pad, w, h) = (self.pad, self.width, self.height);
        let Rows { rows, cells, .. } = &mut self.rows;
        let fixed: f64 = rows.iter().map(|r| if let Height::Fixed(h) = r.height { h } else { 0.0 }).sum();
        let fills = rows.iter().filter(|r| r.height == Height::Fill).count();
        // the gaps: the pad between rows, the list gap before and between fill rows
        let mut gaps = pad * 2.0;
        for k in 1..rows.len() {
            gaps += if rows[k].height == Height::Fill { self.list_gap } else { pad };
        }
        let fill_h = if fills > 0 { ((h - fixed - gaps) / fills as f64).max(0.0) } else { 0.0 };
        let mut y = pad;
        for (k, row) in rows.iter().enumerate() {
            if k > 0 {
                y += if row.height == Height::Fill { self.list_gap } else { pad };
            }
            let rh = match row.height {
                Height::Fixed(h) => h,
                Height::Fill => fill_h,
            };
            let cells = &mut cells[row.cells.clone()];
            let n = cells.len();
            let fixed_w: f64 = cells.iter().map(|c| if let Width::Fixed(w) = c.width { w } else { 0.0 }).sum();
            let flex: f32 = cells.iter().map(|c| if let Width::Flex(f) = c.width { f } else { 0.0 }).sum();
            let free = (w - pad * (n as f64 + 1.0) - fixed_w).max(0.0);
            let mut x = pad;
            for c in cells.iter_mut() {
                let cw = match c.width {
                    Width::Fixed(w) => w,
                    Width::Flex(f) => if flex > 0.0 { free * (f / flex) as f64 } else { 0.0 },
                };
                c.rect = Rect { pos: dvec2(x, y), size: dvec2(cw, rh) };
                x += cw + pad;
            }
            y += rh;
        }
    }

    /// The pressable cell at (u, v) in 0..1 of the card.
    pub fn cell_at(&self, uv: Vec2f) -> Option<u32> {
        let p = dvec2(uv.x as f64 * self.width, uv.y as f64 * self.height);
        self.rows.cells.iter().find(|c| c.kind.pressable() && c.rect.contains(p)).map(|c| c.id)
    }

    /// The pressable cell a ray points at on `card`.
    pub fn hit(&self, card: &Card, origin: Vec3f, dir: Vec3f) -> Option<u32> {
        card.hit(origin, dir).and_then(|(uv, _)| self.cell_at(uv))
    }

    /// The pressable cell a fingertip touches on `card`.
    pub fn poke(&self, card: &Card, tip: Vec3f) -> Option<u32> {
        card.poke(tip).and_then(|uv| self.cell_at(uv))
    }

    fn tone(&self, t: Tone) -> Vec4f {
        match t {
            Tone::Normal => self.color_text,
            Tone::Meta => self.color_meta,
            Tone::Error => self.color_error,
            Tone::Warning => self.color_warning,
            Tone::Info => self.color_info,
            Tone::Good => self.color_good,
        }
    }

    /// The texture, redrawn when what it shows changed: a child pass of the pass being drawn
    /// (call inside the XR pass, before the card is drawn).
    pub fn render(&mut self, cx: &mut Cx2d) {
        if self.tex.is_none() {
            let size = (self.width as usize, self.height as usize);
            let texture = Texture::new_with_format(cx.cx, TextureFormat::RenderBGRAu8 { size: TextureSize::Fixed { width: size.0, height: size.1 }, initial: true });
            let pass = DrawPass::new_with_name(cx.cx, "xr panel");
            pass.set_color_texture(cx.cx, &texture, DrawPassClearColor::ClearWith(vec4(0.0, 0.0, 0.0, 0.0)));
            pass.set_size(cx.cx, dvec2(self.width, self.height));
            self.tex = Some(PanelTexture { pass, list: DrawList::new(cx.cx), texture });
            self.dirty = true;
        }
        if !self.dirty {
            return;
        }
        self.dirty = false;
        let tex = self.tex.as_mut().unwrap();
        cx.make_child_pass(&tex.pass);
        cx.begin_pass(&tex.pass, Some(1.0));
        tex.list.begin_always(cx);
        cx.begin_turtle(Walk::fixed(self.width, self.height), Layout::default());
        self.draw_bg.color = self.color_bg;
        self.draw_bg.draw_abs(cx, Rect { pos: dvec2(0.0, 0.0), size: dvec2(self.width, self.height) });
        let font = self.draw_text.text_style.font_size as f64;
        for k in 0..self.rows.cells.len() {
            let c = &self.rows.cells[k];
            let (kind, rect, id, tone, range) = (c.kind, c.rect, c.id, c.tone, c.text.clone());
            if kind == CellKind::Blank {
                continue;
            }
            if kind.pressable() {
                let base = if kind == CellKind::Toggle(true) { self.color_on } else { self.color_cell };
                self.draw_bg.color = if self.hover == Some(id) {
                    if kind == CellKind::Toggle(true) { base * 1.25 } else { self.color_cell_hover }
                } else {
                    base
                };
                self.draw_bg.draw_abs(cx, rect);
            }
            // what fits the cell (an estimate of the glyphs' advance)
            let text = &self.rows.text[range];
            let fits = (((rect.size.x - 24.0).max(0.0) / (font * 0.62)) as usize).max(1);
            let text = &text[..text.char_indices().nth(fits).map_or(text.len(), |(b, _)| b)];
            let shown = text.chars().count() as f64;
            let x = if kind == CellKind::Heading { rect.pos.x + (rect.size.x - shown * font * 0.55) * 0.5 } else { rect.pos.x + 12.0 };
            self.draw_text.color = self.tone(tone);
            self.draw_text.draw_abs(cx, dvec2(x, rect.pos.y + rect.size.y * 0.5 - font * 0.64), text);
        }
        cx.end_turtle();
        let tex = self.tex.as_mut().unwrap();
        tex.list.end(cx);
        cx.end_pass(&tex.pass);
    }

    /// The card with this panel's picture, in a 3D scene.
    pub fn draw(&mut self, cx: &mut Cx3d, card: &Card) {
        let Some(tex) = &self.tex else { return };
        self.draw_card.draw_vars.set_texture(0, &tex.texture);
        self.draw_card.transform = card.matrix();
        self.draw_card.cube_pos = vec3f(0.0, 0.0, 0.0);
        self.draw_card.cube_size = vec3f(card.size.x, card.size.y, 0.003);
        self.draw_card.draw(cx);
    }

    /// As [`XrPanel::draw`], the card's room pose taken through `room_to_scene` first (a pass
    /// whose scene is not the room).
    pub fn draw_in(&mut self, cx: &mut Cx3d, card: &Card, room_to_scene: &Mat4f) {
        let Some(tex) = &self.tex else { return };
        self.draw_card.draw_vars.set_texture(0, &tex.texture);
        self.draw_card.transform = Mat4f::mul(room_to_scene, &card.matrix());
        self.draw_card.cube_pos = vec3f(0.0, 0.0, 0.0);
        self.draw_card.cube_size = vec3f(card.size.x, card.size.y, 0.003);
        self.draw_card.draw(cx);
    }
}
