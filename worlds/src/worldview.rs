 // a bunch o buttons to select the world
use makepad_render::*; 
use makepad_widget::*; 
use crate::treeworld::TreeWorld;

#[derive(Clone)]
pub struct WorldSelect {
}

#[derive(Clone)]
pub enum WorldSelectEvent {
    SelectWorld(WorldType),
    None,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Hash, Ord)]
pub enum WorldType {
    TreeWorld,
}

impl WorldType {
    fn name(&self) -> String {
        match self {
            Self::TreeWorld => "TreeWorld".to_string(),
        }
    }
}

impl WorldSelect {
    pub fn new(_cx: &mut Cx) -> Self {
        Self {
        }
    }
    
    pub fn style(_cx:&mut Cx){
        
    }
}

#[derive(Clone)]
pub struct WorldView {
    pub select_view: ScrollView,
    pub buttons: Elements<WorldType, NormalButton, NormalButton>,
    pub view: View,
    pub world_type: WorldType,
    pub tree_world: TreeWorld
}

impl WorldView {
    pub fn new(cx: &mut Cx) -> Self {
        Self { 
            view: View::new(cx),
            select_view: ScrollView::new(cx),
            buttons: Elements::new(NormalButton {
                ..NormalButton::new(cx)
            }),
            world_type: WorldType::TreeWorld,
            tree_world: TreeWorld::new(cx)
        }
    }
    
    pub fn style(_cx:&mut Cx){
        
    }

     pub fn handle_world_select(&mut self, cx: &mut Cx, event: &mut Event)->WorldSelectEvent {
            // do shit here
            if self.select_view.handle_scroll_view(cx, event) {
            }
            for (world_type, btn) in self.buttons.enumerate() {
                match btn.handle_normal_button(cx, event) {
                    ButtonEvent::Clicked => {
                        self.world_type = *world_type;
                        self.view.redraw_view_area(cx);
                        return WorldSelectEvent::SelectWorld(*world_type)
                    },
                    _ => ()
                }
            }
            WorldSelectEvent::None
        }
    
    pub fn draw_world_select(&mut self, cx: &mut Cx) {
        if self.select_view.begin_view(cx, Layout::default()).is_err() {return}
        
        let world_types = vec![WorldType::TreeWorld];
        
        for world_type in world_types {
            self.buttons.get_draw(cx, world_type.clone(), | _cx, templ | {
                templ.clone()
            }).draw_normal_button(cx, &world_type.name());
        }
        
        self.select_view.end_view(cx);
    }
    
    pub fn handle_world_view(&mut self, cx: &mut Cx, event: &mut Event) {
        // do shit here
        match &self.world_type{
            WorldType::TreeWorld=>{
                self.tree_world.handle_tree_world(cx, event);
            },
        }
    }
    
    pub fn draw_world_view(&mut self, cx: &mut Cx) {
        // if vr_ispresenting we should simply show a gray rect
        // otherwise we should spawn a 3D viewport
        
        if self.view.begin_view(cx, Layout::default()).is_err() {return}
        match &self.world_type{
            WorldType::TreeWorld=>{
                self.tree_world.draw_tree_world(cx);
            }
        }
        self.view.end_view(cx);
        
    }
}


