use {
    crate::{
        id::{Id, IdMap},
        splitter::{self, Splitter},
        tab_bar::{self, TabBar, TabId},
    },
    makepad_render::*,
};

pub struct Dock {
    panels_by_panel_id: IdMap<PanelId, Panel>,
    panel_id_stack: Vec<PanelId>,
    drag_view: View,
    drag_quad: DrawColor,
}

impl Dock {
    pub fn new(cx: &mut Cx) -> Dock {
        Dock {
            panels_by_panel_id: IdMap::new(),
            panel_id_stack: Vec::new(),
            drag_view: View::new(),
            drag_quad: DrawColor::new(cx, default_shader!()).with_draw_depth(10.0),
        }
    }

    pub fn begin_split_panel(&mut self, cx: &mut Cx, panel_id: PanelId) -> Result<(), ()> {
        let panel = self.get_or_create_split_panel(cx, panel_id);
        panel.splitter.begin(cx)?;
        self.panel_id_stack.push(panel_id);
        Ok(())
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
        self.get_or_create_tab_panel(cx, panel_id);
        self.panel_id_stack.push(panel_id);
    }

    pub fn end_tab_panel(&mut self, cx: &mut Cx) {
        let panel_id = self.panel_id_stack.pop().unwrap();
        let panel = self.panels_by_panel_id[panel_id].as_tab_panel_mut();
        
        // If the panel has a drag state, it's currently being dragged over, so try to draw the drag quad:
        if let Some(drag_state) = panel.drag_state {
            if self.drag_view.begin_view(cx, Layout::default()).is_ok() {
                self.drag_quad.color = vec4(1.0, 1.0, 1.0, 0.5);
                // The drag rectangle is computed in the function contents, which is called either from
                // begin_tab_bar (if the tab bar is not dirty), or end_tab_bar (if the tab bar is dirty).
                self.drag_quad.draw_quad_abs(cx, panel.drag_rect);
                self.drag_view.end_view(cx);
            }
        }
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

    fn contents(&mut self, cx: &mut Cx) {
        let panel_id = *self.panel_id_stack.last().unwrap();
        let panel = self.panels_by_panel_id[panel_id].as_tab_panel_mut();
        cx.turtle_new_line();
        panel.drag_rect = Rect {
            pos: cx.get_turtle_pos(),
            size: Vec2 {
                x: cx.get_width_left(),
                y: cx.get_height_left(),
            }
        };
    }

    fn get_or_create_split_panel(&mut self, cx: &mut Cx, panel_id: PanelId) -> &mut SplitPanel {
        if !self.panels_by_panel_id.contains(panel_id) {
            self.panels_by_panel_id
                .insert(panel_id, Panel::Split(SplitPanel {
                    splitter: Splitter::new(cx)
                }));
        }
        self.panels_by_panel_id[panel_id].as_split_panel_mut()
    }

    fn get_or_create_tab_panel(&mut self, cx: &mut Cx, panel_id: PanelId) -> &mut TabPanel {
        if !self.panels_by_panel_id.contains(panel_id) {
            self.panels_by_panel_id
                .insert(panel_id, Panel::Tab(TabPanel {
                    tab_bar: TabBar::new(cx),
                    drag_rect: Rect::default(),
                    drag_state: None,
                }));
        }
        self.panels_by_panel_id[panel_id].as_tab_panel_mut()
    }

    pub fn selected_tab_id(&mut self, cx: &mut Cx, panel_id: PanelId) -> Option<TabId> {
        let panel = self.get_or_create_tab_panel(cx, panel_id);
        panel.tab_bar.selected_tab_id()
    }

    pub fn set_selected_tab_id(&mut self, cx: &mut Cx, panel_id: PanelId, tab_id: Option<TabId>) {
        let panel = self.get_or_create_tab_panel(cx, panel_id);
        panel.tab_bar.set_selected_tab_id(cx, tab_id);
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
        for panel in &mut self.panels_by_panel_id {
            match panel {
                Panel::Split(panel) => {
                    panel.splitter.handle_event(cx, event, &mut |cx, action| match action {
                        splitter::Action::Redraw => {
                            cx.redraw_child_area(Area::All);
                        }
                    });
                }
                Panel::Tab(panel) => {
                    panel.tab_bar.handle_event(cx, event, &mut |cx, action| match action {
                        tab_bar::Action::TabWasPressed(tab_id) => {
                            dispatch_action(cx, Action::TabWasPressed(tab_id))
                        }
                        tab_bar::Action::TabButtonWasPressed(tab_id) => {
                            dispatch_action(cx, Action::TabButtonWasPressed(tab_id))
                        }
                    });
                    match event {
                        Event::FingerDrag(event) => {
                            let rect = panel.drag_rect;
                            let new_drag_state = if rect.contains(event.abs) {
                                let top_left = rect.pos;
                                let bottom_right = rect.pos + rect.size;
                                Some(if (event.abs.x - top_left.x) / rect.size.x < 0.1 {
                                    DragState::Left
                                } else if (bottom_right.x - event.abs.x) / rect.size.x < 0.1 {
                                    DragState::Right
                                } else if (event.abs.y - top_left.y) / rect.size.y < 0.1 {
                                    DragState::Top
                                } else if (bottom_right.y - event.abs.y) / rect.size.y < 0.1 {
                                    DragState::Bottom
                                } else {
                                    DragState::Center
                                })
                            } else {
                                None
                            };
                            panel.drag_state = new_drag_state;
                            self.drag_view.redraw_view(cx);
                        }
                        _ => {}
                    }
                }
            }
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

    fn as_tab_panel_mut(&mut self) -> &mut TabPanel {
        match self {
            Panel::Tab(panel) => panel,
            _ => panic!(),
        }
    }
}

struct SplitPanel {
    splitter: Splitter
}

struct TabPanel {
    tab_bar: TabBar,
    drag_rect: Rect,
    drag_state: Option<DragState>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum DragState {
    Left,
    Right,
    Top,
    Bottom,
    Center,
}

pub enum Action {
    TabWasPressed(TabId),
    TabButtonWasPressed(TabId),
}
