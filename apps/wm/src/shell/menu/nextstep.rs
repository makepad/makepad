//! NeXT's menu palette: attached columns, and a transient copy under the
//! secondary button. Navigation and command dispatch reuse the shell model.
use super::*;
use super::super::ui::HAlign;

const WIDTH: f64 = 200.0;
const TITLE: f64 = 24.0;
const ROW: f64 = 24.0;

#[derive(Default)]
pub(super) struct NextMenus {
    ancestors: Vec<MenuModel>,
    anchor: Option<Vec2d>,
    cards: Vec<Rect>,
    rows: Vec<Vec<(usize, Rect)>>,
    tracking: bool,
    pressed: bool,
    highlight: bool,
    drag: Option<Vec2d>,
}

fn fit_card(screen: Rect, anchor: Vec2d, count: usize, parent: Option<Rect>) -> Rect {
    let width = WIDTH.min(screen.size.x);
    let capacity = ((screen.size.y - TITLE - 2.0) / ROW).floor().max(0.0);
    let height = (TITLE + ROW * (count.max(1) as f64).min(capacity) + 2.0).min(screen.size.y);
    let mut x = anchor.x;
    let mut y = anchor.y;
    if let Some(parent) = parent {
        x = parent.pos.x + parent.size.x - 1.0;
        if x + width > screen.pos.x + screen.size.x {
            x = parent.pos.x - width + 1.0;
        }
        y = parent.pos.y;
    }
    rect(
        x.clamp(screen.pos.x, screen.pos.x + screen.size.x - width),
        y.clamp(screen.pos.y, screen.pos.y + screen.size.y - height),
        width,
        height,
    )
}

impl ShellMenu {
    pub fn open_next_popup(&mut self, cx: &mut Cx, cursor: Vec2d) {
        self.open_at(cx, "workspace", MenuSkin::Menu);
        // The pointer starts in the title; holding and dragging chooses a row.
        self.next.anchor = Some(cursor - dvec2(18.0, TITLE * 0.5));
        self.next.tracking = true;
    }

    pub(super) fn draw_next_menus(&mut self, cx: &mut Cx2d, screen: Rect) {
        self.next.cards.clear();
        self.next.rows.clear();
        let anchor = self.next.anchor.unwrap_or(screen.pos + dvec2(10.0, 42.0));
        for depth in 0..=self.next.ancestors.len() {
            let model = if depth < self.next.ancestors.len() {
                &self.next.ancestors[depth]
            } else {
                &self.model
            };
            let card = fit_card(screen, anchor, model.rows.len(), self.next.cards.last().copied());
            let (heading, _) = model.header();
            let title = if model.filter.is_empty() {
                heading.trim_end_matches('…').to_string()
            } else {
                heading
            };
            let rows = model.rows.clone();
            let first = model.scroll;
            let selected = model.sel;
            self.d.solid(cx, card, super::super::rgb(64, 64, 64));
            let header = rect(card.pos.x + 1.0, card.pos.y + 1.0, card.size.x - 2.0, TITLE - 1.0);
            self.d.solid(cx, header, super::super::rgb(0, 0, 0));
            self.d.label_elided(cx, rect(header.pos.x + 6.0, header.pos.y, header.size.x - 12.0, header.size.y), true, 13.0, super::super::rgb(255, 255, 255), HAlign::Left, &title);
            let mut hits = Vec::new();
            let capacity = ((card.size.y - TITLE - 2.0) / ROW).floor() as usize;
            for (index, row) in rows.iter().enumerate().skip(first).take(capacity) {
                let r = rect(card.pos.x + 1.0, card.pos.y + TITLE + (index - first) as f64 * ROW, card.size.x - 2.0, ROW);
                let selected = selected == index && !row.disabled
                    && (depth < self.next.ancestors.len() || self.next.highlight);
                let face = if selected { 255 } else { 190 };
                self.d.solid(cx, r, super::super::rgb(face, face, face));
                self.d.solid(cx, rect(r.pos.x, r.pos.y, r.size.x, 1.0), super::super::rgb(238, 238, 238));
                self.d.solid(cx, rect(r.pos.x, r.pos.y, 1.0, r.size.y), super::super::rgb(238, 238, 238));
                self.d.solid(cx, rect(r.pos.x, r.pos.y + r.size.y - 1.0, r.size.x, 1.0), super::super::rgb(85, 85, 85));
                let ink = if row.disabled { super::super::rgb(100, 100, 100) } else { super::super::rgb(0, 0, 0) };
                self.d.label_elided(cx, rect(r.pos.x + 7.0, r.pos.y, r.size.x - 30.0, r.size.y), false, 13.0, ink, HAlign::Left, &row.label);
                if row.has_children {
                    self.d.icon_centered(cx, Ico::ChevronRight, rect(r.pos.x + r.size.x - 21.0, r.pos.y, 16.0, r.size.y), 10.0, ink);
                }
                hits.push((index, r));
            }
            if rows.is_empty() {
                self.d.label(cx, rect(card.pos.x + 1.0, card.pos.y + TITLE, card.size.x - 2.0, ROW), false, 12.0, super::super::rgb(190, 190, 190), HAlign::Center, "No matches");
            }
            self.next.cards.push(card);
            self.next.rows.push(hits);
        }
        self.card = self.next.cards.last().copied().unwrap_or_default();
    }

    /// Windows 2000 retains its parent menu while a child opens alongside
    /// the selected row. The same model stack owns keyboard and mouse routing.
    pub(super) fn draw_classic_menus(&mut self, cx: &mut Cx2d, screen: Rect) {
        use super::super::rgb;
        let row_h = 30.0;
        let width = 244.0f64.min(screen.size.x);
        let root = self.next.ancestors.first().unwrap_or(&self.model);
        let root_h = (root.rows.len() as f64 * row_h + 6.0).min(screen.size.y - 34.0);
        let mut origin = dvec2(screen.pos.x + 3.0, screen.pos.y + screen.size.y - 34.0 - root_h);
        self.next.cards.clear(); self.next.rows.clear();
        for depth in 0..=self.next.ancestors.len() {
            let model = self.next.ancestors.get(depth).unwrap_or(&self.model);
            let rows = model.rows.clone(); let first = model.scroll; let selected = model.sel;
            let capacity = ((screen.size.y - 8.0) / row_h).floor().max(1.0) as usize;
            let height = (rows.len().max(1).min(capacity) as f64 * row_h + 6.0).min(screen.size.y);
            let card = rect(origin.x.clamp(screen.pos.x, screen.pos.x + screen.size.x - width), origin.y.clamp(screen.pos.y, screen.pos.y + screen.size.y - height), width, height);
            let gray = rgb(212,208,200);
            self.d.solid(cx, card, gray);
            for (inset, light, dark) in [(0.0,rgb(255,255,255),rgb(64,64,64)),(1.0,gray,rgb(128,128,128))] {
                self.d.solid(cx, rect(card.pos.x+inset,card.pos.y+inset,width-inset*2.0,1.0),light);
                self.d.solid(cx, rect(card.pos.x+inset,card.pos.y+inset,1.0,height-inset*2.0),light);
                self.d.solid(cx, rect(card.pos.x+inset,card.pos.y+height-inset-1.0,width-inset*2.0,1.0),dark);
                self.d.solid(cx, rect(card.pos.x+width-inset-1.0,card.pos.y+inset,1.0,height-inset*2.0),dark);
            }
            let mut hits = Vec::new();
            for (index,row) in rows.iter().enumerate().skip(first).take(capacity) {
                let r = rect(card.pos.x+3.0,card.pos.y+3.0+(index-first) as f64*row_h,width-6.0,row_h);
                let on = selected==index && !row.disabled && (depth<self.next.ancestors.len() || self.next.highlight);
                let ink = if row.disabled {rgb(128,128,128)} else if on {rgb(255,255,255)} else {rgb(0,0,0)};
                if on {self.d.solid(cx,r,rgb(0,0,128));}
                if row.kind == MenuKind::App {
                    self.app_icons.draw(cx,row.target.strip_prefix("apps.").unwrap_or("app"),DesktopStyle::Windows2000,rect(r.pos.x+5.0,r.pos.y+7.0,16.0,16.0),1.0,ink);
                } else if let Some(icon) = row.icon {
                    self.d.icon_centered(cx,icon,rect(r.pos.x+4.0,r.pos.y,20.0,row_h),16.0,ink);
                }
                self.d.label_elided(cx,rect(r.pos.x+30.0,r.pos.y,r.size.x-50.0,row_h),false,13.0,ink,HAlign::Left,&row.label);
                if row.has_children {self.d.icon_centered(cx,Ico::ChevronRight,rect(r.pos.x+r.size.x-18.0,r.pos.y,14.0,row_h),9.0,ink);}
                hits.push((index,r));
            }
            origin = dvec2(if card.pos.x + width*2.0 > screen.pos.x+screen.size.x {card.pos.x-width+1.0} else {card.pos.x+width-1.0}, card.pos.y+3.0+selected.saturating_sub(first) as f64*row_h);
            self.next.cards.push(card); self.next.rows.push(hits);
        }
        self.card = self.next.cards.last().copied().unwrap_or_default();
    }

    fn cascade_capacity(&self, height: f64) -> usize {
        let (title,row) = if self.desktop_style == DesktopStyle::Windows2000 {(6.0,30.0)} else {(TITLE+2.0,ROW)};
        ((height-title)/row).floor().max(1.0) as usize
    }

    fn next_row_at(&self, p: Vec2d) -> Option<(usize, usize)> {
        // An overlapping child owns its header as well as its rows.
        let depth = self.next.cards.iter().rposition(|card| contains(*card, p))?;
        self.next.rows.get(depth)?.iter().find(|(_, r)| contains(*r, p))
            .map(|(index, _)| (depth, *index))
    }

    fn next_restore_parent(&mut self, depth: usize) {
        if depth < self.next.ancestors.len() {
            self.model = self.next.ancestors[depth].clone();
            self.next.ancestors.truncate(depth);
        }
        self.next.cards.truncate(depth + 1);
        self.next.rows.truncate(depth + 1);
    }

    fn next_choose(&mut self, cx: &mut Cx, depth: usize, index: usize, descend: bool) {
        if let Some(parent) = self.next.ancestors.get(depth) {
            if parent.sel == index {
                return; // Keep the existing attached submenu while crossing to it.
            }
        }
        self.next_restore_parent(depth);
        self.model.sel = index;
        self.next.highlight = true;
        if descend && self.model.rows.get(index).is_some_and(|r| r.has_children && !r.disabled) {
            self.next_activate(cx);
        }
    }

    fn next_activate(&mut self, cx: &mut Cx) {
        let Some(row) = self.model.rows.get(self.model.sel) else { return };
        if row.disabled { return; }
        if row.has_children {
            let parent = self.model.clone();
            self.model.activate();
            self.next.ancestors.push(parent);
            self.next.highlight = false;
        } else if let Some(target) = self.model.activate() {
            self.close(cx);
            cx.widget_action(self.uid, ShellMenuAction::Activate(target));
        }
    }

    fn next_cancel(&mut self, cx: &mut Cx) {
        self.close(cx);
        cx.widget_action(self.uid, ShellMenuAction::Cancel);
    }

    pub(super) fn next_pointer(&mut self, cx: &mut Cx, event: &Event) -> bool {
        match event {
            Event::MouseMove(e) => {
                if let Some(offset) = self.next.drag {
                    self.next.anchor = Some(e.abs - offset);
                } else if self.gate.moved(e.abs) {
                    if let Some((depth, index)) = self.next_row_at(e.abs) {
                        let descend = self.next.tracking || self.next.pressed || !self.next.ancestors.is_empty();
                        self.next_choose(cx, depth, index, descend);
                    }
                }
            }
            Event::MouseDown(e) => {
                if e.button.contains(MouseButton::PRIMARY) {
                    self.next.pressed = true;
                    if let Some((depth, index)) = self.next_row_at(e.abs) {
                        self.next_choose(cx, depth, index, true);
                    } else if let Some(card) = self.next.cards.first().copied().filter(|r| contains(*r, e.abs)) {
                        self.next.drag = Some(e.abs - card.pos);
                    } else if !self.next.cards.iter().any(|r| contains(*r, e.abs)) {
                        self.next_cancel(cx);
                    }
                } else if !self.next.tracking {
                    self.next_cancel(cx);
                }
            }
            Event::MouseUp(e) => {
                let transient = self.next.tracking && e.button.contains(MouseButton::SECONDARY);
                let clicked = self.next.pressed && e.button.contains(MouseButton::PRIMARY);
                if transient || clicked {
                    let dragging = self.next.drag.take().is_some();
                    self.next.pressed = false;
                    if !dragging {
                        if let Some((depth, index)) = self.next_row_at(e.abs) {
                            self.next_restore_parent(depth);
                            self.model.sel = index;
                            self.next_activate(cx);
                        }
                    }
                    if transient && self.model.open { self.next_cancel(cx); }
                }
            }
            Event::Scroll(e) => {
                if let Some(depth) = self.next.cards.iter().rposition(|r| contains(*r, e.abs)) {
                    let card = self.next.cards[depth];
                    self.next_restore_parent(depth);
                    let capacity = self.cascade_capacity(card.size.y);
                    let max = self.model.rows.len().saturating_sub(capacity);
                    if e.scroll.y > 0.5 { self.model.scroll = (self.model.scroll + 1).min(max); }
                    else if e.scroll.y < -0.5 { self.model.scroll = self.model.scroll.saturating_sub(1); }
                }
            }
            _ => return false,
        }
        self.redraw(cx);
        true
    }

    pub(super) fn next_key(&mut self, cx: &mut Cx, e: &KeyEvent) -> bool {
        match e.key_code {
            KeyCode::Escape => self.next_cancel(cx),
            KeyCode::ArrowLeft | KeyCode::Backspace if self.model.filter.is_empty() => {
                if let Some(parent) = self.next.ancestors.pop() { self.model = parent; }
                else { self.next_cancel(cx); }
            }
            KeyCode::Backspace => {
                self.model.filter.pop(); self.model.rebuild(); self.model.scroll = 0;
            }
            KeyCode::ArrowUp => self.model.move_sel(-1),
            KeyCode::ArrowDown => self.model.move_sel(1),
            KeyCode::ReturnKey | KeyCode::NumpadEnter => self.next_activate(cx),
            KeyCode::ArrowRight => {
                if self.model.rows.get(self.model.sel).is_some_and(|r| r.has_children) {
                    self.next_activate(cx);
                }
            }
            key => {
                if e.modifiers.control || e.modifiers.logo || e.modifiers.alt { return true; }
                if let Some(ch) = key.to_char(e.modifiers.shift).filter(|c| !c.is_control()) {
                    self.model.filter.push(ch);
                    self.model.sel = 0; self.model.scroll = 0; self.model.rebuild();
                }
            }
        }
        self.next.highlight = true;
        let capacity = self.cascade_capacity(self.screen.size.y);
        if self.model.sel < self.model.scroll { self.model.scroll = self.model.sel; }
        else if self.model.sel >= self.model.scroll + capacity {
            self.model.scroll = self.model.sel + 1 - capacity;
        }
        self.gate.reset();
        self.redraw(cx);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn popup_and_attached_submenus_stay_inside_the_screen() {
        let screen = rect(0.0, 32.0, 1400.0, 868.0);
        let parent = fit_card(screen, dvec2(1380.0, 880.0), 4, None);
        let child = fit_card(screen, parent.pos, 20, Some(parent));
        assert!(child.pos.x < parent.pos.x);
        for card in [parent, child] {
            assert!(card.pos.x >= screen.pos.x && card.pos.y >= screen.pos.y);
            assert!(card.pos.x + card.size.x <= screen.pos.x + screen.size.x);
            assert!(card.pos.y + card.size.y <= screen.pos.y + screen.size.y);
        }
    }

    #[test]
    fn workspace_routes_reuse_real_apps_and_style_actions() {
        let mut model = MenuModel::default();
        model.open_at("workspace", MenuSkin::Menu);
        assert_eq!(model.activate().as_deref(), Some("apps.files"));
        model.sel = 1;
        assert_eq!(model.activate(), None);
        assert_eq!(model.path, "apps");
        assert!(model.rows.iter().any(|row| row.target == "apps.photos" && !row.disabled));
        assert!(model.back());
        model.sel = 3;
        model.activate();
        assert_eq!(model.path, "desktop");
        assert!(model.rows.iter().any(|row| row.target == "desktop.nextstep"));
    }
}
