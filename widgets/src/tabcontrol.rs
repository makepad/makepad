use render::*;

use crate::scrollbar::*;
use crate::tab::*;

#[derive(Clone, Element)]
pub struct TabControl{
    pub tab_dock_height:f32,

    pub tabs_view:View<ScrollBar>,
    pub tabs:Elements<Tab, usize>,
    pub drag_tab:Element<Tab>,
    pub hover:Quad,
    pub animator:Animator,

    pub _tab_id:usize
}

pub trait TabControlLike{
    fn handle_tab_control(&mut self, cx:&mut Cx, event:&mut Event)->TabControlEvent;
    fn begin_tabs(&mut self, cx:&mut Cx);
    fn draw_tab(&mut self, cx:&mut Cx, title:&str, selected:bool);
    fn end_tabs(&mut self, cx:&mut Cx);
    fn begin_tab_page(&mut self, cx:&mut Cx);
    fn end_tab_page(&mut self, cx:&mut Cx);
}

#[derive(Clone, PartialEq)]
pub enum TabControlState{
    Default,
    Hovering
}

#[derive(Clone, PartialEq)]
pub enum TabControlEvent{
    None,
    Select{new_tab:usize},
}

impl Style for TabControl{
    fn style(cx:&mut Cx)->Self{
        Self{
            tab_dock_height:30.0,

            tabs_view:View{
                scroll_h:Some(Element::new(ScrollBar{
                    bar_size:4.0,
                    ..Style::style(cx)
                })),
                ..Style::style(cx)
            },
            tabs:Elements::new(Tab{
                ..Style::style(cx)
            }),
            drag_tab:Element::new(Tab{
                ..Style::style(cx)
            }),
            hover:Quad{
                color:color("purple"),
                ..Style::style(cx)
            },
            animator:Animator::new(Anim::new(AnimMode::Cut{duration:0.5}, vec![])),

            _tab_id:0,
        }
    }
}

impl TabControlLike for TabControl{
    fn handle_tab_control(&mut self, cx:&mut Cx, event:&mut Event)->TabControlEvent{
        let mut ret_event = TabControlEvent::None;
        for tab in self.tabs.all(){
            tab.handle_tab(cx, event);
        }
        //match event.hits(cx, self.split_area, &mut self.hit_state){
        //    Event::Animate(ae)=>{
        //    },
        //    _=>()
        //};
        ret_event
   }

    // data free APIs for the win!
    fn begin_tabs(&mut self, cx:&mut Cx){
        //cx.begin_turtle(&Layout{
        self.tabs_view.begin_view(cx, &Layout{
            width:Bounds::Fill,
            height:Bounds::Compute,
           ..Default::default()
        });
        //self.tabs.mark();
        self._tab_id = 0;
    }

    fn draw_tab(&mut self, cx:&mut Cx, title:&str, selected:bool){
        let tab = self.tabs.get(cx, self._tab_id);
        self._tab_id += 1;
        tab.draw_tab(cx, title);
    }

    fn end_tabs(&mut self, cx:&mut Cx){
        self.tabs_view.end_view(cx);
    }
    fn begin_tab_page(&mut self, cx:&mut Cx){
        cx.turtle_new_line();
        cx.begin_turtle(&Layout{
            width:Bounds::Fill,
            height:Bounds::Fill,
            ..Default::default()
        })
    }
    fn end_tab_page(&mut self, cx:&mut Cx){
        cx.end_turtle();
    }
}
