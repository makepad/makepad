 // a bunch o buttons to select the world
use makepad_render::*; 
use crate::skybox::SkyBox;

#[derive(Clone)]
pub struct TreeWorld {
    pub view: View,
    pub sky_box: SkyBox
}

impl TreeWorld {
    pub fn new(cx: &mut Cx) -> Self {
        Self { 
            view: View::new(cx),
            sky_box: SkyBox::new(cx)
        }
    }

    pub fn style(_cx:&mut Cx){
        
    }
    
    pub fn handle_tree_world(&mut self, _cx: &mut Cx, _event: &mut Event) {
        // do shit here
        
    }
    
    pub fn draw_tree_world(&mut self, cx: &mut Cx) {
        if self.view.begin_view(cx, Layout::default()).is_err() {return}
       // self.sky_box.draw_sky_box(cx);
        
        self.view.end_view(cx);
    }
}


 
