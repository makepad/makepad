use {
    crate::{
        id::Id,
        id_map::IdMap,
        splitter::{self, Splitter},
        tab_bar::{self, TabBar, TabId},
    },
    makepad_render::*,
};

pub struct Dock {
    view: View,
    panels_by_panel_id: IdMap<PanelId, Panel>,
    panel_ids: Vec<PanelId>,
    panel_id_stack: Vec<PanelId>,
    drag: Option<Drag>,
    drag_view: View,
    drag_quad: DrawColor,
}

impl Dock {
    pub fn style(cx: &mut Cx) {
        live_body!(cx, {
            self::drag_quad_color: #FFFFFF80;
        })
    }

    pub fn new(cx: &mut Cx) -> Dock {
        Dock {
            view: View::new(),
            panels_by_panel_id: IdMap::new(),
            panel_ids: Vec::new(),
            panel_id_stack: Vec::new(),
            drag: None,
            drag_view: View::new().with_is_overlay(true),
            drag_quad: DrawColor::new(cx, default_shader!()).with_draw_depth(10.0),
        }
    }

    pub fn begin(&mut self, cx: &mut Cx) -> Result<(), ()> {
        self.view.begin_view(cx, Layout::default())?;
        self.apply_style(cx);
        self.panel_ids.clear();
        Ok(())
    }

    pub fn end(&mut self, cx: &mut Cx) {
        if self
            .drag_view
            .begin_view(cx, Layout::abs_origin_zero())
            .is_ok()
        {
            if let Some(drag) = self.drag.as_ref() {
                let panel = self.panels_by_panel_id[drag.panel_id].as_tab_panel();
                let rect = compute_drag_rect(panel.contents_rect, drag.position);
                self.drag_quad.draw_quad_abs(cx, rect);
            }
            self.drag_view.end_view(cx);
        }
        self.view.end_view(cx);
    }

    pub fn begin_split_panel(&mut self, cx: &mut Cx, panel_id: PanelId) {
        self.panel_ids.push(panel_id);
        let panel = self.get_or_create_split_panel(cx, panel_id);
        panel.splitter.begin(cx);
        self.panel_id_stack.push(panel_id);
    }

    pub fn middle_split_panel(&mut self, cx: &mut Cx) {
        let panel_id = *self.panel_id_stack.last().unwrap();
        let panel = self.panels_by_panel_id[panel_id].as_split_panel_mut();
        panel.splitter.middle(cx);
    }

    pub fn end_split_panel(&mut self, cx: &mut Cx) {
        let panel_id = self.panel_id_stack.pop().unwrap();
        let panel = self.panels_by_panel_id[panel_id].as_split_panel_mut();
        panel.splitter.end(cx);
    }

    pub fn begin_tab_panel(&mut self, cx: &mut Cx, panel_id: PanelId) {
        self.panel_ids.push(panel_id);
        self.get_or_create_tab_panel(cx, panel_id);
        self.panel_id_stack.push(panel_id);
    }

    pub fn end_tab_panel(&mut self, _cx: &mut Cx) {
        let _ = self.panel_id_stack.pop().unwrap();
    }

    pub fn begin_tab_bar(&mut self, cx: &mut Cx) -> Result<(), ()> {
        let panel_id = *self.panel_id_stack.last().unwrap();
        let panel = self.panels_by_panel_id[panel_id].as_tab_panel_mut();
        if let Err(error) = panel.tab_bar.begin(cx) {
            self.contents(cx);
            return Err(error);
        }
        Ok(())
    }

    pub fn end_tab_bar(&mut self, cx: &mut Cx) {
        let panel_id = *self.panel_id_stack.last().unwrap();
        let panel = self.panels_by_panel_id[panel_id].as_tab_panel_mut();
        panel.tab_bar.end(cx);
        self.contents(cx);
    }

    pub fn tab(&mut self, cx: &mut Cx, tab_id: TabId, name: &str) {
        let panel_id = *self.panel_id_stack.last().unwrap();
        let panel = self.panels_by_panel_id[panel_id].as_tab_panel_mut();
        panel.tab_bar.tab(cx, tab_id, name);
    }

    pub fn set_split_panel_axis(&mut self, cx: &mut Cx, panel_id: PanelId, axis: Axis) {
        let panel = self.get_or_create_split_panel(cx, panel_id);
        panel.splitter.set_axis(axis);
        self.redraw(cx);
    }

    pub fn apply_style(&mut self, cx: &mut Cx) {
        self.drag_quad.color = live_vec4!(cx, self::drag_quad_color);
    }

    fn contents(&mut self, cx: &mut Cx) {
        let panel_id = *self.panel_id_stack.last().unwrap();
        let panel = self.panels_by_panel_id[panel_id].as_tab_panel_mut();
        cx.turtle_new_line();
        panel.contents_rect = Rect {
            pos: cx.get_turtle_pos(),
            size: Vec2 {
                x: cx.get_width_left(),
                y: cx.get_height_left(),
            },
        };
    }

    fn get_or_create_split_panel(&mut self, cx: &mut Cx, panel_id: PanelId) -> &mut SplitPanel {
        if !self.panels_by_panel_id.contains(panel_id) {
            self.panels_by_panel_id.insert(
                panel_id,
                Panel::Split(SplitPanel {
                    splitter: Splitter::new(cx),
                }),
            );
        }
        self.panels_by_panel_id[panel_id].as_split_panel_mut()
    }

    fn get_or_create_tab_panel(&mut self, cx: &mut Cx, panel_id: PanelId) -> &mut TabPanel {
        if !self.panels_by_panel_id.contains(panel_id) {
            self.panels_by_panel_id.insert(
                panel_id,
                Panel::Tab(TabPanel {
                    tab_bar: TabBar::new(cx),
                    contents_rect: Rect::default(),
                }),
            );
        }
        self.panels_by_panel_id[panel_id].as_tab_panel_mut()
    }

    pub fn forget(&mut self) {
        self.panels_by_panel_id.clear();
    }

    pub fn forget_panel_id(&mut self, panel_id: PanelId) {
        self.panels_by_panel_id.remove(panel_id);
    }

    pub fn selected_tab_id(&mut self, cx: &mut Cx, panel_id: PanelId) -> Option<TabId> {
        let panel = self.get_or_create_tab_panel(cx, panel_id);
        panel.tab_bar.selected_tab_id()
    }

    pub fn set_selected_tab_id(&mut self, cx: &mut Cx, panel_id: PanelId, tab_id: Option<TabId>) {
        let panel = self.get_or_create_tab_panel(cx, panel_id);
        panel.tab_bar.set_selected_tab_id(cx, tab_id);
    }

    pub fn redraw(&mut self, cx: &mut Cx) {
        self.view.redraw_view(cx);
    }

    pub fn redraw_tab_bar(&mut self, cx: &mut Cx, panel_id: PanelId) {
        let panel = self.get_or_create_tab_panel(cx, panel_id);
        panel.tab_bar.redraw(cx);
    }

    pub fn handle_event(
        &mut self,
        cx: &mut Cx,
        event: &mut Event,
        dispatch_action: &mut dyn FnMut(&mut Cx, Action),
    ) {
        for panel_id in &self.panel_ids {
            let panel = &mut self.panels_by_panel_id[*panel_id];
            match panel {
                Panel::Split(panel) => {
                    panel
                        .splitter
                        .handle_event(cx, event, &mut |cx, action| match action {
                            splitter::Action::DidChange => {
                                dispatch_action(cx, Action::SplitPanelDidChange(*panel_id));
                            }
                        });
                }
                Panel::Tab(panel) => {
                    panel
                        .tab_bar
                        .handle_event(cx, event, &mut |cx, action| match action {
                            tab_bar::Action::TabWasPressed(tab_id) => {
                                dispatch_action(cx, Action::TabWasPressed(tab_id))
                            }
                            tab_bar::Action::TabButtonWasPressed(tab_id) => {
                                dispatch_action(cx, Action::TabButtonWasPressed(tab_id))
                            }
                        });
                }
            }
        }
        match event {
            Event::FingerDrag(event) => {
                self.drag = None;
                for panel_id in &self.panel_ids {
                    let panel = &mut self.panels_by_panel_id[*panel_id];
                    if let Panel::Tab(panel) = panel {
                        if panel.contents_rect.contains(event.abs) {
                            event.action = DragAction::Copy;
                            self.drag = Some(Drag {
                                panel_id: *panel_id,
                                position: compute_drag_position(panel.contents_rect, event.abs),
                            });
                        }
                    }
                }
                self.drag_view.redraw_view(cx);
            }
            Event::FingerDrop(event) => {
                self.drag = None;
                for panel_id in &self.panel_ids {
                    let panel = &mut self.panels_by_panel_id[*panel_id];
                    if let Panel::Tab(panel) = panel {
                        if panel.contents_rect.contains(event.abs) {
                            dispatch_action(
                                cx,
                                Action::TabPanelDidReceiveDraggedItem(
                                    *panel_id,
                                    compute_drag_position(panel.contents_rect, event.abs),
                                    event.dragged_item.clone(),
                                ),
                            );
                        }
                    }
                }
                self.drag_view.redraw_view(cx);
            }
            _ => {}
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct PanelId(pub Id);

impl AsRef<Id> for PanelId {
    fn as_ref(&self) -> &Id {
        &self.0
    }
}

enum Panel {
    Split(SplitPanel),
    Tab(TabPanel),
}

impl Panel {
    fn as_split_panel_mut(&mut self) -> &mut SplitPanel {
        match self {
            Panel::Split(panel) => panel,
            _ => panic!(),
        }
    }

    fn as_tab_panel(&self) -> &TabPanel {
        match self {
            Panel::Tab(panel) => panel,
            _ => panic!(),
        }
    }

    fn as_tab_panel_mut(&mut self) -> &mut TabPanel {
        match self {
            Panel::Tab(panel) => panel,
            _ => panic!(),
        }
    }
}

struct SplitPanel {
    splitter: Splitter,
}

struct TabPanel {
    tab_bar: TabBar,
    contents_rect: Rect,
}

struct Drag {
    panel_id: PanelId,
    position: DragPosition,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DragPosition {
    Left,
    Right,
    Top,
    Bottom,
    Center,
}

pub enum Action {
    SplitPanelDidChange(PanelId),
    TabWasPressed(TabId),
    TabButtonWasPressed(TabId),
    TabPanelDidReceiveDraggedItem(PanelId, DragPosition, DraggedItem),
}

fn compute_drag_position(rect: Rect, position: Vec2) -> DragPosition {
    let top_left = rect.pos;
    let bottom_right = rect.pos + rect.size;
    if (position.x - top_left.x) / rect.size.x < 0.1 {
        DragPosition::Left
    } else if (bottom_right.x - position.x) / rect.size.x < 0.1 {
        DragPosition::Right
    } else if (position.y - top_left.y) / rect.size.y < 0.1 {
        DragPosition::Top
    } else if (bottom_right.y - position.y) / rect.size.y < 0.1 {
        DragPosition::Bottom
    } else {
        DragPosition::Center
    }
}

fn compute_drag_rect(rect: Rect, position: DragPosition) -> Rect {
    match position {
        DragPosition::Left => Rect {
            pos: rect.pos,
            size: Vec2 {
                x: rect.size.x / 2.0,
                y: rect.size.y,
            },
        },
        DragPosition::Right => Rect {
            pos: Vec2 {
                x: rect.pos.x + rect.size.x / 2.0,
                y: rect.pos.y,
            },
            size: Vec2 {
                x: rect.size.x / 2.0,
                y: rect.size.y,
            },
        },
        DragPosition::Top => Rect {
            pos: rect.pos,
            size: Vec2 {
                x: rect.size.x,
                y: rect.size.y / 2.0,
            },
        },
        DragPosition::Bottom => Rect {
            pos: Vec2 {
                x: rect.pos.x,
                y: rect.pos.y + rect.size.y / 2.0,
            },
            size: Vec2 {
                x: rect.size.x,
                y: rect.size.y / 2.0,
            },
        },
        DragPosition::Center => rect,
    }
}
