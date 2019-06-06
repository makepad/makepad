use render::*;
use crate::scrollbar::*;

#[derive(Clone)]
pub struct DesktopWindow{
    pub window:Window,
    pub layout:Layout,
    pub root_view:View<ScrollBar>, // we have a root view otherwise is_overlay subviews can't attach topmost
    pub inner_view:View<ScrollBar>, 
}

#[derive(Clone, PartialEq)]
pub enum DesktopWindowEvent{
    EventForOtherWindow,
    WindowClosed,
    WindowGeomChange(WindowGeomChangeEvent),
    None
}

impl Style for DesktopWindow{
    fn style(cx:&mut Cx)->Self{
        Self{
            layout:Layout{
                ..Default::default()
            },
            root_view:View{
                ..Style::style(cx)
            },
            inner_view:View{
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
        self.root_view.handle_scroll_bars(cx, event);
        self.inner_view.handle_scroll_bars(cx, event);
        if let Some(window_id) = self.window.window_id{
            let is_for_other_window = match event{
                Event::WindowCloseRequested(ev)=>ev.window_id != window_id,
                Event::WindowClosed(ev)=>{
                    if ev.window_id == window_id{
                        return DesktopWindowEvent::WindowClosed
                    }
                    true
                }
                Event::WindowGeomChange(ev)=>{
                    if ev.window_id == window_id{
                        return DesktopWindowEvent::WindowGeomChange(ev.clone())
                    }
                    true
                },
                Event::FingerDown(ev)=>ev.window_id != window_id,
                Event::FingerMove(ev)=>ev.window_id != window_id,
                Event::FingerHover(ev)=>ev.window_id != window_id,
                Event::FingerUp(ev)=>ev.window_id != window_id,
                Event::FingerScroll(ev)=>ev.window_id != window_id,
                _=>false
            };
            if is_for_other_window{
                DesktopWindowEvent::EventForOtherWindow
            }
            else{
                DesktopWindowEvent::None
            }
        }
        else{
            DesktopWindowEvent::None
        }
    }

    pub fn begin_desktop_window(&mut self, cx:&mut Cx)->ViewRedraw{

        self.window.begin_window(cx);
        if let Err(_) = self.root_view.begin_view(cx, &Layout{..Default::default()}){
            self.window.end_window(cx);
            return Err(())
        }
        if let Err(_) = self.inner_view.begin_view(cx, &self.layout){
            self.root_view.end_view(cx);
            self.window.end_window(cx);
            return Err(())
        }

        Ok(())
    }
    
    pub fn end_desktop_window(&mut self, cx:&mut Cx){
        self.inner_view.end_view(cx);
        self.root_view.end_view(cx);
        self.window.end_window(cx);
    }
}

