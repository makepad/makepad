use render::*;
use crate::scrollbar::*;

#[derive(Clone)]
pub struct DesktopWindow{
    pub window:Window,
    pub layout:Layout,
    pub view:View<ScrollBar>,
}

#[derive(Clone, PartialEq)]
pub enum DesktopWindowEvent{
    EventForOtherWindow,
    None
}

impl Style for DesktopWindow{
    fn style(cx:&mut Cx)->Self{
        Self{
            layout:Layout{
                ..Default::default()
            },
            view:View{
                //scroll_h:Some(ScrollBar{
                //    ..Style::style(cx)
                //}),
                //scroll_v:Some(ScrollBar{
                //    smoothing:Some(0.25),
                //    ..Style::style(cx)
                //}),
                ..Style::style(cx)
            },
            window:Window{
                ..Style::style(cx)
            }
        }
    }
}

impl DesktopWindow{
    pub fn handle_desktop_window(&mut self, cx:&mut Cx, event:&mut Event)->DesktopWindowEvent{
        self.view.handle_scroll_bars(cx, event);
        DesktopWindowEvent::None
    }

    pub fn begin_desktop_window(&mut self, cx:&mut Cx)->ViewRedraw{

        self.window.begin_window(cx);
        if let Err(_) = self.view.begin_view(cx, &self.layout){
            self.window.end_window(cx);
            return Err(())
        }

        Ok(())
    }
    
    pub fn end_desktop_window(&mut self, cx:&mut Cx){
        self.view.end_view(cx);
        self.window.end_window(cx);
    }
}

