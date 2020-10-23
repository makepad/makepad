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
    
    pub fn style(_cx: &mut Cx) {
        
    }
}

#[derive(Clone)]
pub struct WorldView {
    pub select_view: ScrollView,
    pub buttons: Elements<WorldType, NormalButton, NormalButton>,
    pub view: View,
    pub viewport_3d: Viewport3D,
    pub world_type: WorldType,
    pub tree_world: TreeWorld
}

impl WorldView {
    pub fn new(cx: &mut Cx) -> Self {
        Self {
            view: View::new(cx),
            select_view: ScrollView::new(cx),
            viewport_3d: Viewport3D::new(cx),
            buttons: Elements::new(NormalButton {
                ..NormalButton::new(cx)
            }),
            world_type: WorldType::TreeWorld,
            tree_world: TreeWorld::new(cx)
        }
    }
    
    pub fn style(cx: &mut Cx) {
        live_body!(cx, r#"
            self::uniforms: ShaderLib {
                uniform time:float;
            }
        "#);
    }
    
    pub fn handle_world_select(&mut self, cx: &mut Cx, event: &mut Event) -> WorldSelectEvent {
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
        
        match event.is_frame_event(cx, self.view.get_view_area(cx)) {
            Event::Frame(ae) => {
                // lets update the shaders
                let areas = match &self.world_type {
                    WorldType::TreeWorld => {
                        vec![self.tree_world.tree_area]
                    }
                };
                for area in areas{ // lets find some uniforms
                    area.write_uniform_float(cx, live_item_id!(self::uniforms::time), ae.time as f32);
                }
                cx.next_frame(self.view.get_view_area(cx));
            },
            _ => ()
        }
        match event {
            Event::LiveRecompile(_) => {
                self.viewport_3d.main_view.redraw_view_area(cx);
            },
            _ => ()
        }
        match &self.world_type {
            WorldType::TreeWorld => {
                self.tree_world.handle_tree_world(cx, event);
            },
        }
    }
    
    pub fn draw_world_view_2d(&mut self, cx: &mut Cx, xr_is_presenting: bool) {
        // we need to draw our wold view in a 3D window here
        if xr_is_presenting {
            // just do some gray rect
            return
        }
        
        if self.viewport_3d.begin_viewport_3d(cx).is_ok() {
            self.draw_world_view_3d(cx);
            self.viewport_3d.end_viewport_3d(cx)
        };
        
        // lets draw it
        self.viewport_3d.draw_viewport_2d(cx);
        
    }
    
    pub fn draw_world_view_3d(&mut self, cx: &mut Cx) {
        
        if self.view.begin_view(cx, Layout::abs_origin_zero()).is_err() {
            return
        };
        
        self.view.block_set_view_transform(cx);
        
        match &self.world_type {
            WorldType::TreeWorld => {
                self.tree_world.draw_tree_world(cx);
            }
        }
        
        self.view.end_view(cx,);
        cx.next_frame(self.view.get_view_area(cx));
    }
    
}


