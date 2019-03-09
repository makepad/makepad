use render::*;

use crate::scrollbar::*;
use crate::tab::*;

#[derive(Clone, Element)]
pub struct TabControl{
    pub tabs_view:View<ScrollBar>,
    pub tabs:Elements<Tab, String>,
    pub drag_tab:Element<Tab>,
    pub hover:Quad,
    pub anims:Anims,

    pub tab_dock_height:f32
}

pub trait TabControlLike{
    fn handle_tab_control(&mut self, cx:&mut Cx, event:&mut Event)->TabControlEvent;
    fn begin_tabs(&mut self, cx:&mut Cx);
    fn draw_tab(&mut self, cx:&mut Cx, title:&str);
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
            anims:Anims::new(Anim::new(AnimMode::Cut{duration:0.5}, vec![])),
        }
    }
}

impl TabControlLike for TabControl{
    fn handle_tab_control(&mut self, cx:&mut Cx, event:&mut Event)->TabControlEvent{
        let mut ret_event = TabControlEvent::None;
        //match event.hits(cx, self.split_area, &mut self.hit_state){
        //    Event::Animate(ae)=>{
        //    },
        //    _=>()
        //};
        ret_event
   }

    // data free APIs for the win!
    fn begin_tabs(&mut self, cx:&mut Cx){
        self.tabs_view.begin_view(cx, &Layout{
            width:Bounds::Fill,
            height:Bounds::Fix(self.tab_dock_height),
            ..Default::default()
        });
    }
    fn draw_tab(&mut self, cx:&mut Cx, title:&str){

    }
    fn end_tabs(&mut self, cx:&mut Cx){
        self.tabs_view.end_view(cx);
    }
    fn begin_tab_page(&mut self, cx:&mut Cx){

    }
    fn end_tab_page(&mut self, cx:&mut Cx){

    }
}
