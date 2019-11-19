use render::*;

use crate::scrollbar::*;
use crate::scrollview::*;
use crate::tab::*;
use crate::widgettheme::*;

#[derive(Clone)]
pub struct TabControl {
    pub tabs_view: ScrollView,
    pub tabs: Elements<usize, Tab, Tab>,
    pub drag_tab_view: View,
    pub drag_tab: Tab,
    pub page_view: View,
    pub hover: Quad,
    //pub tab_fill_color: ColorId,
    pub tab_fill: Quad,
    pub animator: Animator,
    
    pub _dragging_tab: Option<(FingerMoveEvent, usize)>,
    pub _tab_id_alloc: usize,
    pub _tab_now_selected: Option<usize>,
    pub _tab_last_selected:Option<usize>,
    pub _focussed: bool
}

#[derive(Clone, PartialEq)]
pub enum TabControlEvent {
    None,
    TabDragMove {fe: FingerMoveEvent, tab_id: usize},
    TabDragEnd {fe: FingerUpEvent, tab_id: usize},
    TabSelect {tab_id: usize},
    TabClose {tab_id: usize}
}

impl TabControl {
    pub fn proto(cx: &mut Cx) -> Self {
        Self {
            tabs_view: ScrollView {
                scroll_h: Some(ScrollBar {
                    bar_size: 8.0,
                    smoothing: Some(0.15),
                    use_vertical_finger_scroll: true,
                    ..ScrollBar::proto(cx)
                }),
                ..ScrollView::proto(cx)
            },
            page_view: View::proto(cx),
            tabs: Elements::new(Tab::proto(cx)),
            drag_tab: Tab {
                z: 10.,
                ..Tab::proto(cx)
            },
            drag_tab_view: View {
                is_overlay: true,
                ..View::proto(cx)
            },
            hover: Quad {
                color: color("purple"),
                ..Quad::proto(cx)
            },
            //tab_fill_color: Color_bg_normal::id(),
            tab_fill: Quad::proto(cx),
            animator: Animator::default(),
            _dragging_tab: None,
            _tab_now_selected:None,
            _tab_last_selected:None,
            _focussed: false,
            _tab_id_alloc: 0
        }
    }
    
    pub fn handle_tab_control(&mut self, cx: &mut Cx, event: &mut Event) -> TabControlEvent {
        let mut tab_control_event = TabControlEvent::None;
        
        self.tabs_view.handle_scroll_bars(cx, event);
        
        for (id, tab) in self.tabs.enumerate() {
            
            match tab.handle_tab(cx, event) {
                TabEvent::Select => {
                    self.page_view.redraw_view_area(cx);
                    // deselect the other tabs
                    tab_control_event = TabControlEvent::TabSelect {tab_id: *id}
                },
                TabEvent::DragMove(fe) => {
                    self._dragging_tab = Some((fe.clone(), *id));
                    // flag our view as dirty, to trigger
                    self.tabs_view.redraw_view_area(cx);
                    self.drag_tab_view.redraw_view_area(cx);
                    
                    tab_control_event = TabControlEvent::TabDragMove {fe: fe, tab_id: *id};
                },
                TabEvent::DragEnd(fe) => {
                    self._dragging_tab = None;
                    self.drag_tab_view.redraw_view_area(cx);
                    
                    tab_control_event = TabControlEvent::TabDragEnd {fe, tab_id: *id};
                },
                TabEvent::Closing => { // this tab is closing. select the visible one
                    if tab._is_selected { // only do anything if we are selected
                        let next_sel = if *id == self._tab_id_alloc - 1 { // last id
                            if *id > 0 {
                                *id - 1
                            }
                            else {
                                *id
                            }
                        }
                        else {
                            *id + 1
                        };
                        if *id != next_sel {
                            tab_control_event = TabControlEvent::TabSelect {tab_id: next_sel};
                        }
                    }
                },
                TabEvent::Close => {
                    // Sooooo someone wants to close the tab
                    tab_control_event = TabControlEvent::TabClose {tab_id: *id};
                },
                _ => ()
            }
        };
        match tab_control_event {
            TabControlEvent::TabSelect {tab_id} => {
                self._focussed = true;
                for (id, tab) in self.tabs.enumerate() {
                    if tab_id != *id {
                        tab.set_tab_selected(cx, false);
                        tab.set_tab_focus(cx, true);
                    }
                }
            },
            TabControlEvent::TabClose {..} => { // needed to clear animation state
                self.tabs.clear(cx, | _, _ | ());
            },
            _ => ()
        };
        tab_control_event
    }
    
    pub fn get_tab_rects(&mut self, cx: &Cx) -> Vec<Rect> {
        let mut rects = Vec::new();
        for tab in self.tabs.iter() {
            rects.push(tab.get_tab_rect(cx))
        }
        return rects
    }
    
    pub fn set_tab_control_focus(&mut self, cx: &mut Cx, focus: bool) {
        self._focussed = focus;
        for tab in self.tabs.iter() {
            tab.set_tab_focus(cx, focus);
        }
    }
    
    pub fn get_tabs_view_rect(&mut self, cx: &Cx) -> Rect {
        self.tabs_view.get_rect(cx)
    }
    
    pub fn get_content_drop_rect(&mut self, cx: &Cx) -> Rect {
        let pr = self.page_view.get_rect(cx);
        // we now need to change the y and the new height
        Rect {
            x: pr.x,
            y: pr.y,
            w: pr.w,
            h: pr.h
        }
    }
    
    // data free APIs for the win!
    pub fn begin_tabs(&mut self, cx: &mut Cx) -> ViewRedraw {
        //cx.begin_turtle(&Layout{
        if let Err(_) = self.tabs_view.begin_view(cx, Layout {
            walk:Walk::wh(Width::Fill, Height::Compute),
            ..Layout::default()
        }) {
            return Err(())
        }
        self._tab_now_selected = None;
        self._tab_id_alloc = 0;
        Ok(())
    }

    pub fn get_draw_tab(&mut self, cx: &mut Cx, label: &str, selected: bool, closeable: bool)->&mut Tab{
        let new_tab = self.tabs.get(self._tab_id_alloc).is_none();
        let tab = self.tabs.get_draw(cx, self._tab_id_alloc, | _cx, tmpl | tmpl.clone());
        if selected{
            self._tab_now_selected = Some(self._tab_id_alloc);
        }
        self._tab_id_alloc += 1;
        tab.label = label.to_string();
        tab.is_closeable = closeable;
        if new_tab {
            tab.set_tab_state(cx, selected, self._focussed);
        }
        else { // animate the tabstate
            tab.set_tab_selected(cx, selected);
        }
        tab
    }

    pub fn draw_tab(&mut self, cx: &mut Cx, label: &str, selected: bool, closeable: bool) {
        let tab = self.get_draw_tab(cx, label, selected, closeable);
        tab.draw_tab(cx);
    }
    
    pub fn end_tabs(&mut self, cx: &mut Cx) {
        self.tab_fill.color = Theme::color_bg_normal().base(cx);
        self.tab_fill.draw_quad(cx, Walk::wh(Width::Fill, Height::Fill));
        self.tabs.sweep(cx, | _, _ | ());
        if let Some((fe, id)) = &self._dragging_tab {
            if let Ok(()) = self.drag_tab_view.begin_view(cx, Layout {
                abs_origin: Some(Vec2::zero()),
                ..Default::default()
            }) {
                
                self.drag_tab.abs_origin = Some(Vec2 {x: fe.abs.x - fe.rel_start.x, y: fe.abs.y - fe.rel_start.y});
                let origin_tab = self.tabs.get_draw(cx, *id, | _cx, tmpl | tmpl.clone());
                self.drag_tab.label = origin_tab.label.clone();
                self.drag_tab.is_closeable = origin_tab.is_closeable;
                self.drag_tab.draw_tab(cx);
                
                self.drag_tab_view.end_view(cx);
            }
        }
        self.tabs_view.end_view(cx);
        if self._tab_now_selected != self._tab_last_selected{
            // lets scroll the thing into view
            if let Some(tab_id) = self._tab_now_selected{
                if let Some(tab) = self.tabs.get(tab_id){
                    let tab_rect = tab._bg_area.get_rect(cx, true);
                    self.tabs_view.scroll_into_view_abs(cx, tab_rect);
                }
            }
            self._tab_last_selected = self._tab_now_selected;
        }
    }
    
    pub fn begin_tab_page(&mut self, cx: &mut Cx) -> ViewRedraw {
        cx.turtle_new_line();
        self.page_view.begin_view(cx, Layout::default())
    }
    
    pub fn end_tab_page(&mut self, cx: &mut Cx) {
        self.page_view.end_view(cx);
        //cx.end_turtle(Area::Empty);
        // if we are in draggable tab state,
        // draw our draggable tab
    }
}
