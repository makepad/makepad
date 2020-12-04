// a bunch o buttons to select the world
use makepad_render::*;
use makepad_widget::*;
use crate::treeworld::TreeWorld;
use crate::fieldworld::FieldWorld;

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
    FieldWorld,
}

impl WorldType {
    fn name(&self) -> String {
        match self {
            Self::TreeWorld => "TreeWorld".to_string(),
            Self::FieldWorld => "FieldWorld".to_string(),
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
    pub bg: DrawColor,
    pub xr_is_presenting: bool,
    pub time: f32,
    pub last_xr_update_event: Option<XRUpdateEvent>,
    pub viewport_3d: Viewport3D,
    pub world_type: WorldType,
    pub tree_world: TreeWorld,
    pub field_world: FieldWorld,
    pub next_frame: NextFrame
}

impl WorldView {
    pub fn new(cx: &mut Cx) -> Self {
        Self {
            view: View::new(),
            bg: DrawColor::new(cx, default_shader!()),
            select_view: ScrollView::new_standard_hv(cx),
            viewport_3d: Viewport3D::new(cx),
            buttons: Elements::new(NormalButton::new(cx)),
            time: 0.0,
            world_type: WorldType::TreeWorld,
            xr_is_presenting: false,
            last_xr_update_event: None, 
            tree_world: TreeWorld::new(cx),
            field_world: FieldWorld::new(cx),
            next_frame:NextFrame::default()
        }
    }
    
    pub fn style(cx: &mut Cx) {
        live_body!(cx, r#"
            self::color_bg: #222222;
            self::uniforms: ShaderLib {
                uniform time:float;
                uniform left_input_pos: vec3;
                uniform right_input_pos: vec3;
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
                    self.view.redraw_view(cx);
                    return WorldSelectEvent::SelectWorld(*world_type)
                },
                _ => ()
            }
        }
        WorldSelectEvent::None
    }
    
    pub fn draw_world_select(&mut self, cx: &mut Cx) {
        if self.select_view.begin_view(cx, Layout::default()).is_err() {return}
        self.bg.color = live_vec4!(cx, self::color_bg);
        self.bg.draw_quad_rel(cx, cx.get_turtle_rect());
        //let inst = self.bg.begin_quad_fill(cx);
       
        let world_types = vec![WorldType::TreeWorld, WorldType::FieldWorld];
        
        for world_type in world_types {
            self.buttons.get_draw(cx, world_type.clone(), | _cx, templ | {
                templ.clone()
            }).draw_normal_button(cx, &world_type.name());
        }
        
        //self.bg.end_quad_fill(cx, inst);
        self.select_view.end_view(cx);
    }
    
    pub fn update_uniforms(&mut self, cx: &mut Cx){
        // lets update the shaders
        let areas = match &self.world_type {
            WorldType::TreeWorld => {
                vec![self.tree_world.area]
            }
            WorldType::FieldWorld => {
                vec![self.field_world.area]
            }
        }; 
        for area in areas{ // lets find some uniforms
            write_draw_input!(cx, area, self::uniforms::time, self.time);
            if let Some(xu) = &self.last_xr_update_event{
                write_draw_input!(cx, area, self::uniforms::left_input_pos, xu.left_input.ray.position);
                write_draw_input!(cx, area, self::uniforms::right_input_pos, xu.right_input.ray.position);
            }  
        }  
    }
    
    pub fn handle_world_view(&mut self, cx: &mut Cx, event: &mut Event) {
        // do 2D camera interaction.
        if !self.xr_is_presenting{
            self.viewport_3d.handle_viewport_2d(cx, event);
        }
        
        if let Some(ae) = event.is_next_frame(cx, self.next_frame) {
            self.time = ae.time as f32;
            self.update_uniforms(cx);
            self.next_frame = cx.new_next_frame();
        } 

        match event {
            Event::XRUpdate(xu)=>{
               self.last_xr_update_event = Some(xu.clone());
            },
            Event::LiveRecompile(_) => {
                self.viewport_3d.view_3d.redraw_view(cx);
            },
            _ => ()
        }
        
        match &self.world_type {
            WorldType::TreeWorld => {
                self.tree_world.handle_tree_world(cx, event);
            },
            WorldType::FieldWorld => {
                self.field_world.handle_field_world(cx, event);
            }, 
        } 
    }
    
    pub fn draw_world_view_2d(&mut self, cx: &mut Cx) {
        // we need to draw our wold view in a 3D window here
        self.xr_is_presenting = cx.is_xr_presenting();
        if self.xr_is_presenting {
            // just do some gray rect
            self.bg.color = live_vec4!(cx, self::color_bg);
            self.bg.draw_quad_abs(cx, cx.get_turtle_rect());
            //let inst = self.bg.begin_quad_fill(cx);
            //self.bg.end_quad_fill(cx, inst);
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
        
        self.view.lock_view_transform(cx, &Mat4::identity());
        
        match &self.world_type {
            WorldType::TreeWorld => {
                self.tree_world.draw_tree_world(cx);
            }
            WorldType::FieldWorld => {
                self.field_world.draw_field_world(cx);
            }
        }
        
        self.view.end_view(cx,);
        self.next_frame = cx.new_next_frame();
        self.update_uniforms(cx);
    }
    
} 


