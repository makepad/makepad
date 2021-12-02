use {
    crate::{
        genid::GenId,
        genid_map::GenIdMap,
        splitter::{SplitterAction, Splitter, SplitterAlign},
        tab_bar::{TabBarAction, TabBar, TabId},
    },
    makepad_render::*,
};

live_register!{
    use crate::tab_bar::TabBar
    use crate::splitter::Splitter 
    Dock: {{Dock}} {
        drag_quad: {
            draw_depth: 10.0
            color: #ffffff80
        }
        drag_view: {
            layout: {abs_origin: vec2(0, 0)}
            is_overlay: true
        }
        tab_bar: TabBar {}
        splitter: Splitter {}
    }
}

#[derive(Live, LiveHook)]
pub struct Dock {
    
    view: View,
    drag_view: View,
    drag_quad: DrawColor,
    tab_bar: Option<LivePtr>,
    splitter: Option<LivePtr>,
    
    #[rust] panels_by_panel_id: GenIdMap<PanelId, Panel>,
    #[rust] panel_ids: Vec<PanelId>,
    #[rust] panel_id_stack: Vec<PanelId>,
    #[rust] drag: Option<Drag>,
}

impl Dock {
    
    pub fn begin(&mut self, cx: &mut Cx) -> Result<(), ()> {
        self.view.begin(cx) ?;
        self.panel_ids.clear();
        Ok(())
    }
    
    pub fn end(&mut self, cx: &mut Cx) {
        if self
        .drag_view
            .begin(cx)
            .is_ok()
        {
            if let Some(drag) = self.drag.as_ref() {
                let panel = self.panels_by_panel_id[drag.panel_id].as_tab_panel();
                let rect = compute_drag_rect(panel.contents_rect, drag.position);
                self.drag_quad.draw_abs(cx, rect);
            }
            self.drag_view.end(cx);
        }
        self.view.end(cx);
    }
    
    pub fn begin_split_panel(&mut self, cx: &mut Cx, panel_id: PanelId, axis:Axis, align:SplitterAlign) {
        self.panel_ids.push(panel_id);
        let panel = self.get_or_create_split_panel(cx, panel_id);
        panel.splitter.set_axis(axis);
        panel.splitter.set_align(align);
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
                    splitter: Splitter::new_from_ptr(cx, self.splitter.unwrap()),
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
                    tab_bar: TabBar::new_from_ptr(cx, self.tab_bar.unwrap()),
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
        self.view.redraw(cx);
    }
    
    pub fn redraw_tab_bar(&mut self, cx: &mut Cx, panel_id: PanelId) {
        let panel = self.get_or_create_tab_panel(cx, panel_id);
        panel.tab_bar.redraw(cx);
    }
    
    pub fn handle_event(
        &mut self,
        cx: &mut Cx,
        event: &mut Event,
        dispatch_action: &mut dyn FnMut(&mut Cx, DockAction),
    ) {
        for panel_id in &self.panel_ids {
            let panel = &mut self.panels_by_panel_id[*panel_id];
            match panel {
                Panel::Split(panel) => {
                    panel
                        .splitter
                        .handle_event(cx, event, &mut | cx, action | match action {
                        SplitterAction::Changed{axis, align} => {
                            dispatch_action(cx, DockAction::SplitPanelChanged{panel_id:*panel_id, axis, align});
                        }
                    });
                }
                Panel::Tab(panel) => {
                    panel
                        .tab_bar
                        .handle_event(cx, event, &mut | cx, action | match action {
                        TabBarAction::ReceivedDraggedItem(item) => dispatch_action(
                            cx,
                            DockAction::TabBarReceivedDraggedItem(*panel_id, item),
                        ),
                        TabBarAction::TabWasPressed(tab_id) => {
                            dispatch_action(cx, DockAction::TabWasPressed(*panel_id, tab_id))
                        }
                        TabBarAction::TabButtonWasPressed(tab_id) => {
                            dispatch_action(cx, DockAction::TabButtonWasPressed(*panel_id, tab_id))
                        }
                        TabBarAction::TabReceivedDraggedItem(tab_id, item) => {
                            dispatch_action(
                                cx,
                                DockAction::TabReceivedDraggedItem(*panel_id, tab_id, item),
                            )
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
                            self.drag = Some(Drag {
                                panel_id: *panel_id,
                                position: compute_drag_position(panel.contents_rect, event.abs),
                            });
                            event.action = DragAction::Copy;
                        }
                    }
                }
                self.drag_view.redraw(cx);
            }
            Event::FingerDrop(event) => {
                self.drag = None;
                for panel_id in &self.panel_ids {
                    let panel = &mut self.panels_by_panel_id[*panel_id];
                    if let Panel::Tab(panel) = panel {
                        if panel.contents_rect.contains(event.abs) {
                            dispatch_action(
                                cx,
                                DockAction::ContentsReceivedDraggedItem(
                                    *panel_id,
                                    compute_drag_position(panel.contents_rect, event.abs),
                                    event.dragged_item.clone(),
                                ),
                            );
                        }
                    }
                }
                self.drag_view.redraw(cx);
            }
            _ => {}
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct PanelId(pub GenId);

impl AsRef<GenId> for PanelId {
    fn as_ref(&self) -> &GenId {
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

pub enum DockAction {
    SplitPanelChanged{panel_id:PanelId, axis:Axis, align:SplitterAlign},
    TabBarReceivedDraggedItem(PanelId, DraggedItem),
    TabWasPressed(PanelId, TabId),
    TabButtonWasPressed(PanelId, TabId),
    TabReceivedDraggedItem(PanelId, TabId, DraggedItem),
    ContentsReceivedDraggedItem(PanelId, DragPosition, DraggedItem),
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
