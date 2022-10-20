use {
    crate::{
        makepad_draw_2d::*,
        splitter::{SplitterAction, Splitter, SplitterAlign},
        tab_bar::{TabBarAction, TabBar, TabId},
    },
};

live_design!{
    import makepad_draw_2d::shader::std::*;
    import crate::tab_bar::TabBar
    import makepad_widgets::splitter::Splitter
    import makepad_widgets::theme::*;
    
    DrawRoundCorner = {{DrawRoundCorner}} {
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
    
    Dock= {{Dock}} {
        const BORDER_SIZE: 6.0
        border_size: (BORDER_SIZE)
        layout: {
            flow: Down
            padding: {left: (BORDER_SIZE), top: 0.0, right: (BORDER_SIZE), bottom: (BORDER_SIZE)}
        }
        padding_fill: {color: (COLOR_BG_APP)}
        drag_quad: {
            draw_depth: 10.0
            color: (COLOR_DRAG_QUAD)
        }
        overlay_view: {
            //walk: {abs_pos: vec2(0.0, 0.0)}
            //is_overlay: true
        }
        tab_bar: <TabBar> {}
        splitter: <Splitter> {}
    }
}

#[derive(Live, LiveHook)]
#[repr(C)]
pub struct DrawRoundCorner {
    draw_super: DrawQuad,
    border_radius: f32,
    flip: Vec2,
}

#[derive(Live)]
pub struct Dock {
    layout: Layout,
    overlay_view: View,
    round_corner: DrawRoundCorner,
    padding_fill: DrawColor,
    border_size: f64,
    drag_quad: DrawColor,
    tab_bar: Option<LivePtr>,
    splitter: Option<LivePtr>,
    #[rust] area: Area,
    #[rust] panels: ComponentMap<PanelId, Panel>,
    #[rust] panel_id_stack: Vec<PanelId>,
    #[rust] drag: Option<Drag>,
}

impl LiveHook for Dock {
    fn after_apply(&mut self, cx: &mut Cx, from: ApplyFrom, index: usize, nodes: &[LiveNode]) {
        for panel in self.panels.values_mut() {
            match panel {
                Panel::Split(panel) => if let Some(index) = nodes.child_by_name(index, live_id!(splitter).as_field()) {
                    panel.splitter.apply(cx, from, index, nodes);
                }
                Panel::Tab(panel) => if let Some(index) = nodes.child_by_name(index, live_id!(tab_bar).as_field()) {
                    panel.tab_bar.apply(cx, from, index, nodes);
                }
            }
        }
        self.area.redraw(cx);
    }
}

impl Dock {
    
    pub fn begin(&mut self, cx: &mut Cx2d) {
        cx.begin_turtle(Walk::default(), self.layout);
    }
    
    pub fn end(&mut self, cx: &mut Cx2d) {
        if self.overlay_view.begin(cx).is_redrawing() {
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
                    let rad = dvec2(rc.border_radius as f64, rc.border_radius as f64);
                    let pos = panel.full_rect.pos;
                    let size = panel.full_rect.size;
                    rc.draw_abs(cx, Rect {pos, size: rad});
                    rc.flip = vec2(1.0, 0.0);
                    rc.draw_abs(cx, Rect {pos: pos + dvec2(size.x - rad.x, 0.), size: rad});
                    rc.flip = vec2(1.0, 1.0);
                    rc.draw_abs(cx, Rect {pos: pos + dvec2(size.x - rad.x, size.y - rad.y), size: rad});
                    rc.flip = vec2(0.0, 1.0);
                    rc.draw_abs(cx, Rect {pos: pos + dvec2(0., size.y - rad.y), size: rad});
                }
                _ => ()
            }
        }
        let pf = &mut self.padding_fill;
        // lets get the turtle rect
        let rect = cx.turtle().rect();

        pf.draw_abs(cx, Rect {
            pos: rect.pos,
            size: dvec2(self.border_size, rect.size.y)
        });
        pf.draw_abs(cx, Rect {
            pos: rect.pos + dvec2(rect.size.x - self.border_size, 0.0),
            size: dvec2(self.border_size, rect.size.y)
        });
        pf.draw_abs(cx, Rect {
            pos: rect.pos + dvec2(0., rect.size.y - self.border_size),
            size: dvec2(rect.size.x, self.border_size)
        });
        cx.end_turtle_with_area(&mut self.area);
    }
    
    pub fn begin_split_panel(&mut self, cx: &mut Cx2d, panel_id: PanelId, axis: Axis, align: SplitterAlign) {
        let panel = self.get_or_create_split_panel(cx, panel_id);
        panel.splitter.set_axis(axis);
        panel.splitter.set_align(align);
        panel.splitter.begin(cx, Walk::default());
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
    
    pub fn begin_tab_bar(&mut self, cx: &mut Cx2d, selected_tab: Option<usize>) {
        let panel_id = *self.panel_id_stack.last().unwrap();
        let panel = self.panels[panel_id].as_tab_panel_mut();
        panel.full_rect = cx.turtle().rect();
        panel.tab_bar.begin(cx, selected_tab);
    }
    
    pub fn end_tab_bar(&mut self, cx: &mut Cx2d) {
        let panel_id = *self.panel_id_stack.last().unwrap();
        let panel = self.panels[panel_id].as_tab_panel_mut();
        panel.tab_bar.end(cx);
        //self.contents(cx);
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
    
    pub fn begin_contents(&mut self, cx: &mut Cx2d)->ViewRedrawing {
        let panel_id = *self.panel_id_stack.last().unwrap();
        let panel = self.panels[panel_id].as_tab_panel_mut();
        panel.contents_view.begin(cx)
    } 
    
    pub fn end_contents(&mut self, cx: &mut Cx2d){
        let panel_id = *self.panel_id_stack.last().unwrap();
        let panel = self.panels[panel_id].as_tab_panel_mut();
        panel.contents_view.end(cx);
    } 
    
    fn get_or_create_split_panel(&mut self, cx: &mut Cx, panel_id: PanelId) -> &mut SplitPanel {
        let splitter = self.splitter;
        self.panels.get_or_insert(cx, panel_id, | cx | {
            Panel::Split(SplitPanel {
                splitter: Splitter::new_from_ptr(cx, splitter),
            })
        }).as_split_panel_mut()
    }
    
    fn get_or_create_tab_panel(&mut self, cx: &mut Cx, panel_id: PanelId) -> &mut TabPanel {
        let tab_bar = self.tab_bar;
        self.panels.get_or_insert(cx, panel_id, | cx | {
            Panel::Tab(TabPanel {
                tab_bar: TabBar::new_from_ptr(cx, tab_bar),
                contents_view: View::new(cx),
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
    
    pub fn redraw(&self, cx: &mut Cx) {
        self.area.redraw(cx);
    }
    
    pub fn redraw_tab_bar(&mut self, cx: &mut Cx, panel_id: PanelId) {
        let panel = self.get_or_create_tab_panel(cx, panel_id);
        panel.tab_bar.redraw(cx);
    }
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event) -> Vec<DockAction> {
        let mut a = Vec::new();
        self.handle_event_with_fn(cx, event, &mut | _, v | a.push(v));
        a
    }
    
    pub fn handle_event_with_fn(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        dispatch_action: &mut dyn FnMut(&mut Cx, DockAction),
    ) {
        for (panel_id, panel) in self.panels.iter_mut() {
            match panel {
                Panel::Split(panel) => {
                    panel
                        .splitter
                        .handle_event_fn(cx, event, &mut | cx, action | match action {
                        SplitterAction::Changed {axis, align} => {
                            dispatch_action(cx, DockAction::SplitPanelChanged {panel_id: *panel_id, axis, align});
                        },
                        _=>()
                    });
                }
                Panel::Tab(panel) => {
                    let mut redraw = false;
                    panel
                        .tab_bar
                        .handle_event_fn(cx, event, &mut | cx, action | match action {
                        TabBarAction::ReceivedDraggedItem(item) => dispatch_action(
                            cx,
                            DockAction::TabBarReceivedDraggedItem(*panel_id, item),
                        ),
                        TabBarAction::TabWasPressed(tab_id) => {
                            redraw = true;
                            dispatch_action(cx, DockAction::TabWasPressed(*panel_id, tab_id))
                        }
                        TabBarAction::TabCloseWasPressed(tab_id) => {
                            redraw = true;
                            dispatch_action(cx, DockAction::TabCloseWasPressed(*panel_id, tab_id))
                        }
                        TabBarAction::TabReceivedDraggedItem(tab_id, item) => {
                            dispatch_action(
                                cx,
                                DockAction::TabReceivedDraggedItem(*panel_id, tab_id, item),
                            )
                        }
                    });
                    if redraw{
                        panel.contents_view.redraw(cx);
                    }
                }
            }
        }
        match event {
            Event::Drag(event) => {
                self.drag = None;
                for (panel_id, panel) in self.panels.iter_mut() {
                    if let Panel::Tab(panel) = panel {
                        if panel.contents_rect.contains(event.abs) {
                            self.drag = Some(Drag {
                                panel_id: *panel_id,
                                position: compute_drag_position(panel.contents_rect, event.abs),
                            });
                            event.action.set(DragAction::Copy);
                        }
                    }
                }
                self.overlay_view.redraw(cx);
            }
            Event::Drop(event) => {
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
    contents_view: View,
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

fn compute_drag_position(rect: Rect, position: DVec2) -> DragPosition {
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
            size: DVec2 {
                x: rect.size.x / 2.0,
                y: rect.size.y,
            },
        },
        DragPosition::Right => Rect {
            pos: DVec2 {
                x: rect.pos.x + rect.size.x / 2.0,
                y: rect.pos.y,
            },
            size: DVec2 {
                x: rect.size.x / 2.0,
                y: rect.size.y,
            },
        },
        DragPosition::Top => Rect {
            pos: rect.pos,
            size: DVec2 {
                x: rect.size.x,
                y: rect.size.y / 2.0,
            },
        },
        DragPosition::Bottom => Rect {
            pos: DVec2 {
                x: rect.pos.x,
                y: rect.pos.y + rect.size.y / 2.0,
            },
            size: DVec2 {
                x: rect.size.x,
                y: rect.size.y / 2.0,
            },
        },
        DragPosition::Center => rect,
    }
}
