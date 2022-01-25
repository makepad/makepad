use {
    crate::{
        makepad_platform::*,
        makepad_component::{
            component_map::ComponentMap
        },
        splitter::{SplitterAction, Splitter, SplitterAlign},
        tab_bar::{TabBarAction, TabBar, TabId},
    },
};

live_register!{
    use makepad_platform::shader::std::*;
    use crate::tab_bar::TabBar
    use crate::splitter::Splitter
    use makepad_component::theme::*;
    
    DrawRoundCorner: {{DrawRoundCorner}} {
        draw_depth: 6.0
        border_radius: 10.0
        fn pixel(self) -> vec4 {
            
            let pos = vec2(
                mix(self.pos.x, 1.0 - self.pos.x, self.flip.x),
                mix(self.pos.y, 1.0 - self.pos.y, self.flip.y)
            )
            
            let sdf = Sdf2d::viewport(pos * self.rect_size);
            sdf.rect(-10., -10., self.rect_size.x * 2.0, self.rect_size.y * 2.0);
            sdf.box(
                0.25,
                0.25,
                self.rect_size.x * 2.0,
                self.rect_size.y * 2.0,
                4.0
            );
            
            sdf.subtract()
            return sdf.fill(COLOR_BG_APP);
        }
    }
    
    Dock: {{Dock}} {
        const BORDER_SIZE: 6.0
        border_size: (BORDER_SIZE)
        view: {
            layout: {padding: {left: (BORDER_SIZE), top: 0.0, right: (BORDER_SIZE), bottom: (BORDER_SIZE)}}
        }
        padding_fill: {color: (COLOR_BG_APP)}
        drag_quad: {
            draw_depth: 10.0
            color: (COLOR_DRAG_QUAD)
        }
        overlay_view: {
            layout: {abs_origin: vec2(0, 0)}
            is_overlay: true
        }
        tab_bar: TabBar {}
        splitter: Splitter {}
    }
}

#[derive(Live, LiveHook)]
#[repr(C)]
pub struct DrawRoundCorner {
    deref_target: DrawQuad,
    border_radius: f32,
    flip: Vec2,
}

#[derive(Live)]
pub struct Dock {
    
    view: View,
    overlay_view: View,
    round_corner: DrawRoundCorner,
    padding_fill: DrawColor,
    border_size: f32,
    drag_quad: DrawColor,
    tab_bar: Option<LivePtr>,
    splitter: Option<LivePtr>,
    
    #[rust] panels: ComponentMap<PanelId, Panel>,
    #[rust] panel_id_stack: Vec<PanelId>,
    #[rust] drag: Option<Drag>,
}

impl LiveHook for Dock {
    fn after_apply(&mut self, cx: &mut Cx, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode]) {
        for panel in self.panels.values_mut() {
            match panel {
                Panel::Split(panel) => if let Some(index) = nodes.child_by_name(index, id!(splitter)) {
                    panel.splitter.apply(cx, apply_from, index, nodes);
                }
                Panel::Tab(panel) => if let Some(index) = nodes.child_by_name(index, id!(tab_bar)) {
                    panel.tab_bar.apply(cx, apply_from, index, nodes);
                }
            }
        }
        self.view.redraw(cx);
    }
}

impl Dock {
    
    pub fn begin(&mut self, cx: &mut Cx2d) -> Result<(), ()> {
        self.view.begin(cx) ?;
        Ok(())
    }
    
    pub fn end(&mut self, cx: &mut Cx2d) {
        if self .overlay_view.begin(cx).is_ok() {
            if let Some(drag) = self.drag.as_ref() {
                let panel = self.panels[drag.panel_id].as_tab_panel();
                let rect = compute_drag_rect(panel.contents_rect, drag.position);
                self.drag_quad.draw_abs(cx, rect);
            }
            self.overlay_view.end(cx);
        }
        self.panels.retain_visible();
        
        // lets draw the borders here
        for panel in self.panels.values() {
            match panel {
                Panel::Tab(panel) => {
                    let rc = &mut self.round_corner;
                    rc.flip = vec2(0.0, 0.0);
                    let rad = vec2(rc.border_radius, rc.border_radius);
                    let pos = panel.full_rect.pos;
                    let size = panel.full_rect.size;
                    rc.draw_abs(cx, Rect {pos, size: rad});
                    rc.flip = vec2(1.0, 0.0);
                    rc.draw_abs(cx, Rect {pos: pos + vec2(size.x - rad.x, 0.), size: rad});
                    rc.flip = vec2(1.0, 1.0);
                    rc.draw_abs(cx, Rect {pos: pos + vec2(size.x - rad.x, size.y - rad.y), size: rad});
                    rc.flip = vec2(0.0, 1.0);
                    rc.draw_abs(cx, Rect {pos: pos + vec2(0., size.y - rad.y), size: rad});
                }
                _ => ()
            }
        }
        let pf = &mut self.padding_fill;
        // lets get the turtle rect
        let rect = cx.get_turtle_rect();
        pf.draw_abs(cx, Rect {
            pos: rect.pos,
            size: vec2(self.border_size, rect.size.y)
        });
        pf.draw_abs(cx, Rect {
            pos: rect.pos + vec2(rect.size.x - self.border_size, 0.0),
            size: vec2(self.border_size, rect.size.y)
        });
        pf.draw_abs(cx, Rect {
            pos: rect.pos + vec2(0., rect.size.y - self.border_size),
            size: vec2(rect.size.x, self.border_size)
        });
        
        self.view.end(cx);
    }
    
    pub fn begin_split_panel(&mut self, cx: &mut Cx2d, panel_id: PanelId, axis: Axis, align: SplitterAlign) {
        let panel = self.get_or_create_split_panel(cx, panel_id);
        panel.splitter.set_axis(axis);
        panel.splitter.set_align(align);
        panel.splitter.begin(cx);
        self.panel_id_stack.push(panel_id);
    }
    
    pub fn middle_split_panel(&mut self, cx: &mut Cx2d) {
        let panel_id = *self.panel_id_stack.last().unwrap();
        let panel = self.panels[panel_id].as_split_panel_mut();
        panel.splitter.middle(cx);
    }
    
    pub fn end_split_panel(&mut self, cx: &mut Cx2d) {
        let panel_id = self.panel_id_stack.pop().unwrap();
        let panel = self.panels[panel_id].as_split_panel_mut();
        panel.splitter.end(cx);
    }
    
    pub fn begin_tab_panel(&mut self, cx: &mut Cx2d, panel_id: PanelId) {
        self.get_or_create_tab_panel(cx, panel_id);
        self.panel_id_stack.push(panel_id);
    }
    
    pub fn end_tab_panel(&mut self, _cx: &mut Cx2d) {
        let _ = self.panel_id_stack.pop().unwrap();
    }
    
    pub fn begin_tab_bar(&mut self, cx: &mut Cx2d, selected_tab: Option<usize>) -> Result<(), ()> {
        let panel_id = *self.panel_id_stack.last().unwrap();
        let panel = self.panels[panel_id].as_tab_panel_mut();
        panel.full_rect = cx.get_turtle_rect();
        
        if let Err(error) = panel.tab_bar.begin(cx, selected_tab) {
            self.contents(cx);
            return Err(error);
        }
        Ok(())
    }
    
    pub fn end_tab_bar(&mut self, cx: &mut Cx2d) {
        let panel_id = *self.panel_id_stack.last().unwrap();
        let panel = self.panels[panel_id].as_tab_panel_mut();
        panel.tab_bar.end(cx);
        self.contents(cx);
    }
    
    pub fn draw_tab(&mut self, cx: &mut Cx2d, tab_id: TabId, name: &str) {
        let panel_id = *self.panel_id_stack.last().unwrap();
        let panel = self.panels[panel_id].as_tab_panel_mut();
        panel.tab_bar.draw_tab(cx, tab_id, name);
    }
    
    pub fn set_split_panel_axis(&mut self, cx: &mut Cx, panel_id: PanelId, axis: Axis) {
        let panel = self.get_or_create_split_panel(cx, panel_id);
        panel.splitter.set_axis(axis);
        self.redraw(cx);
    }
    
    fn contents(&mut self, cx: &mut Cx2d) {
        let panel_id = *self.panel_id_stack.last().unwrap();
        let panel = self.panels[panel_id].as_tab_panel_mut();
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
        let splitter = self.splitter;
        self.panels.get_or_insert(cx, panel_id, | cx | {
            Panel::Split(SplitPanel {
                splitter: Splitter::new_from_option_ptr(cx, splitter),
            })
        }).as_split_panel_mut()
    }
    
    fn get_or_create_tab_panel(&mut self, cx: &mut Cx, panel_id: PanelId) -> &mut TabPanel {
        let tab_bar = self.tab_bar;
        self.panels.get_or_insert(cx, panel_id, | cx | {
            Panel::Tab(TabPanel {
                tab_bar: TabBar::new_from_option_ptr(cx, tab_bar),
                contents_rect: Rect::default(),
                full_rect: Rect::default(),
            })
        }).as_tab_panel_mut()
    }
    
    
    pub fn selected_tab_id(&mut self, cx: &mut Cx, panel_id: PanelId) -> Option<TabId> {
        let panel = self.get_or_create_tab_panel(cx, panel_id);
        panel.tab_bar.selected_tab_id()
    }
    
    pub fn set_selected_tab_id(&mut self, cx: &mut Cx, panel_id: PanelId, tab_id: Option<TabId>, animate: Animate) {
        let panel = self.get_or_create_tab_panel(cx, panel_id);
        panel.tab_bar.set_selected_tab_id(cx, tab_id, animate);
    }
    
    pub fn set_next_selected_tab(&mut self, cx: &mut Cx, panel_id: PanelId, tab_id: TabId, animate: Animate) {
        let panel = self.get_or_create_tab_panel(cx, panel_id);
        panel.tab_bar.set_next_selected_tab(cx, tab_id, animate);
    }
    
    pub fn redraw(&mut self, cx: &mut Cx) {
        self.view.redraw(cx);
    }
    
    pub fn redraw_tab_bar(&mut self, cx: &mut Cx, panel_id: PanelId) {
        let panel = self.get_or_create_tab_panel(cx, panel_id);
        panel.tab_bar.redraw(cx);
    }

    pub fn handle_event(&mut self, cx:&mut Cx, event:&mut Event)->Vec<DockAction>{
        let mut a = Vec::new(); self.handle_event_with_fn(cx, event, &mut|_, v| a.push(v)); a
    }
    
    pub fn handle_event_with_fn(
        &mut self,
        cx: &mut Cx,
        event: &mut Event,
        dispatch_action: &mut dyn FnMut(&mut Cx, DockAction),
    ) {
        for (panel_id, panel) in self.panels.iter_mut() {
            match panel {
                Panel::Split(panel) => {
                    panel
                        .splitter
                        .handle_event(cx, event, &mut | cx, action | match action {
                        SplitterAction::Changed {axis, align} => {
                            dispatch_action(cx, DockAction::SplitPanelChanged {panel_id: *panel_id, axis, align});
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
                        TabBarAction::TabCloseWasPressed(tab_id) => {
                            dispatch_action(cx, DockAction::TabCloseWasPressed(*panel_id, tab_id))
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
                for (panel_id, panel) in self.panels.iter_mut() {
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
                self.overlay_view.redraw(cx);
            }
            Event::FingerDrop(event) => {
                self.drag = None;
                for (panel_id, panel) in self.panels.iter_mut() {
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
                self.overlay_view.redraw(cx);
            }
            _ => {}
        }
    }
}

#[derive(Clone, Debug, Default, Eq, Hash, Copy, PartialEq, FromLiveId)]
pub struct PanelId(pub LiveId);

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
    full_rect: Rect
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
    SplitPanelChanged {panel_id: PanelId, axis: Axis, align: SplitterAlign},
    TabBarReceivedDraggedItem(PanelId, DraggedItem),
    TabWasPressed(PanelId, TabId),
    TabCloseWasPressed(PanelId, TabId),
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
