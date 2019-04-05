use render::*;

use crate::scrollbar::*;
use crate::tab::*;

#[derive(Clone, Element)]
pub struct TabControl{
    pub tabs_view:View<ScrollBar>,
    pub tabs:Elements<Tab, usize>,
    pub drag_tab_view:View<NoScrollBar>,
    pub drag_tab:Tab,
    pub page_view:View<NoScrollBar>,
    pub hover:Quad,
    pub tab_fill:Quad,
    pub animator:Animator,

    pub _dragging_tab:Option<(FingerMoveEvent,usize)>,
    pub _tab_id_alloc:usize,
    pub _focussed:bool
}

#[derive(Clone, PartialEq)]
pub enum TabControlEvent{
    None,
    TabDragMove{fe:FingerMoveEvent, tab_id:usize},
    TabDragEnd{fe:FingerUpEvent, tab_id:usize},
    TabSelect{tab_id:usize},
    TabClose{tab_id:usize}
}

impl Style for TabControl{
    fn style(cx:&mut Cx)->Self{
        Self{
            tabs_view:View{
                scroll_h:Some(ScrollBar{
                    bar_size:8.0,
                    ..Style::style(cx)
                }),
                ..Style::style(cx)
            },
            page_view:View{
                ..Style::style(cx)
            },
            tabs:Elements::new(Tab{
                ..Style::style(cx)
            }),
            drag_tab:Tab{
                ..Style::style(cx)
            },
            drag_tab_view:View{
                is_overlay:true,
                ..Style::style(cx)
            },
            hover:Quad{
                color:color("purple"),
                ..Style::style(cx)
            },
            tab_fill:Quad{
                color:cx.color("bg_normal"),
                ..Style::style(cx)
            },
            animator:Animator::new(Anim::new(Play::Cut{duration:0.5}, vec![])),
            _dragging_tab:None,
            _focussed:false,
            _tab_id_alloc:0
        }
    }
}

impl TabControl{
    pub fn handle_tab_control(&mut self, cx:&mut Cx, event:&mut Event)->TabControlEvent{
        let mut tab_control_event = TabControlEvent::None;

        self.tabs_view.handle_scroll_bars(cx, event);

        for (id, tab) in self.tabs.enumerate(){
            match tab.handle_tab(cx, event){
                TabEvent::Select=>{
                   self.page_view.redraw_view_area(cx);
                    // deselect the other tabs
                   tab_control_event = TabControlEvent::TabSelect{tab_id:*id}
                },
                TabEvent::DragMove(fe)=>{
                    self._dragging_tab = Some((fe.clone(), *id));
                    // flag our view as dirty, to trigger
                    cx.redraw_area(self.tabs_view.get_view_area(cx));

                    tab_control_event = TabControlEvent::TabDragMove{fe:fe, tab_id:*id};
                },
                TabEvent::DragEnd(fe)=>{
                    self._dragging_tab = None;
                    cx.redraw_area(self.tabs_view.get_view_area(cx));

                    tab_control_event = TabControlEvent::TabDragEnd{fe, tab_id:*id};
                },
                TabEvent::Closing=>{ // this tab is closing. select the visible one
                    if tab._is_selected{ // only do anything if we are selected
                        let next_sel = if *id == self._tab_id_alloc - 1{ // last id
                            if *id > 0{
                                *id - 1
                            }
                            else{
                                *id
                            }
                        }
                        else{
                            *id + 1
                        };
                        if *id != next_sel{
                            tab_control_event = TabControlEvent::TabSelect{tab_id:next_sel};
                        }
                    }
                },
                TabEvent::Close=>{
                    // Sooooo someone wants to close the tab
                    tab_control_event = TabControlEvent::TabClose{tab_id:*id};
                },
                _=>()
            }
        };
        match tab_control_event{
            TabControlEvent::TabSelect{tab_id}=>{
                self._focussed = true;
                for (id, tab) in self.tabs.enumerate(){
                    if tab_id != *id{
                        tab.set_tab_selected(cx, false);
                        tab.set_tab_focus(cx, true);
                    }
                }
            },
            TabControlEvent::TabClose{..}=>{ // needed to clear animation state
                self.tabs.clear(cx);
            },
            _=>()
        };
        tab_control_event
    }
    
    pub fn get_tab_rects(&mut self, cx:&Cx)->Vec<Rect>{
        let mut rects = Vec::new();
        for tab in self.tabs.iter(){
            rects.push(tab.get_tab_rect(cx))
        }
        return rects
    }

    pub fn set_tab_control_focus(&mut self, cx:&mut Cx, focus:bool){
        self._focussed = focus;
        for tab in self.tabs.iter(){
            tab.set_tab_focus(cx, focus);
        }
    }

    pub fn get_tabs_view_rect(&mut self, cx:&Cx)->Rect{
        self.tabs_view.get_view_area(cx).get_rect(cx)
    }

    pub fn get_content_drop_rect(&mut self, cx:&Cx)->Rect{
        let pr = self.page_view.get_view_area(cx).get_rect(cx);
        // we now need to change the y and the new height
        Rect{
            x:pr.x,
            y:pr.y,
            w:pr.w,
            h:pr.h
        }
    }

    // data free APIs for the win!
    pub fn begin_tabs(&mut self, cx:&mut Cx){
        //cx.begin_turtle(&Layout{
        self.tabs_view.begin_view(cx, &Layout{
            width:Bounds::Fill,
            height:Bounds::Compute,
           ..Default::default()
        });
        //self.tabs.mark();
        self._tab_id_alloc = 0;
    }

    pub fn draw_tab(&mut self, cx:&mut Cx, label:&str, selected:bool, closeable:bool){
        let new_tab = self.tabs.get(self._tab_id_alloc).is_none();
        let tab = self.tabs.get_draw(cx, self._tab_id_alloc);
        self._tab_id_alloc += 1;
        tab.label = label.to_string();
        tab.is_closeable = closeable;
        if new_tab{
            tab.set_tab_state(cx, selected, self._focussed);
        }
        else{ // animate the tabstate
            tab.set_tab_selected(cx, selected);
        }
        tab.draw_tab(cx);
    }

    pub fn end_tabs(&mut self, cx:&mut Cx){
        self.tab_fill.draw_quad_walk(cx, Bounds::Fill, Bounds::Fill, Margin::zero());
        self.tabs.sweep(cx);
        if let Some((fe, id)) = &self._dragging_tab{
            self.drag_tab_view.begin_view(cx, &Layout{
                abs_start:Some(vec2(0.,0.)),
                ..Default::default()
            });
            
            self.drag_tab.bg_layout.abs_start = Some(vec2(fe.abs.x - fe.rel_start.x, fe.abs.y - fe.rel_start.y));
            let origin_tab = self.tabs.get_draw(cx, *id);
            self.drag_tab.label = origin_tab.label.clone();
            self.drag_tab.is_closeable = origin_tab.is_closeable;
            self.drag_tab.draw_tab(cx);

            self.drag_tab_view.end_view(cx);
        }
        self.tabs_view.end_view(cx);
    }

    pub fn begin_tab_page(&mut self, cx:&mut Cx){
        cx.turtle_new_line();
        self.page_view.begin_view(cx, &Layout{..Default::default()});
    }

    pub fn end_tab_page(&mut self, cx:&mut Cx){
        self.page_view.end_view(cx);
        //cx.end_turtle(Area::Empty);
        // if we are in draggable tab state,
        // draw our draggable tab
    }
}
