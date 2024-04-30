use crate::{
    makepad_derive_widget::*,
    makepad_draw::*,
    designer_data::*,
    turtle_step::*,
    view::View,
    widget::*,
};

live_design!{
    DesignerViewBase = {{DesignerView}}{
    }
    
    DesignerContainerBase = {{DesignerContainer}}{
    }
    
}

struct ContainerData{
    component: WidgetRef,
    container: WidgetRef,
    rect: Rect
}

enum Edge{
    Left,
    Right,
    Bottom,
    Top,
}

impl ContainerData{
    fn get_edge(&self, rel:DVec2, zoom:f64, pan: DVec2)->Option<Edge>{
        let cp = rel * zoom + pan;
        let edge_outer:f64 = 5.0 * zoom ;
        let edge_inner:f64  = 5.0 * zoom ;
                
        if cp.x >= self.rect.pos.x - edge_outer && 
        cp.x <= self.rect.pos.x + edge_inner && 
        cp.y >= self.rect.pos.y && 
        cp.y <= self.rect.pos.y + self.rect.size.y{
            // left edge
            return Some(Edge::Left);
        }
        if cp.x >= self.rect.pos.x + self.rect.size.x- edge_outer && 
        cp.x <= self.rect.pos.x + self.rect.size.x+ edge_inner && 
        cp.y >= self.rect.pos.y && 
        cp.y <= self.rect.pos.y + self.rect.size.y{
            return Some(Edge::Right);
        }
        else if cp.y >= self.rect.pos.y - edge_outer && 
        cp.y <= self.rect.pos.y + edge_inner &&
        cp.x >= self.rect.pos.x && 
        cp.x <= self.rect.pos.x + self.rect.size.x{
            // top edge
            return Some(Edge::Top);
        }
        else if cp.y >= self.rect.pos.y + self.rect.size.y- edge_outer && 
        cp.y <= self.rect.pos.y + self.rect.size.y + edge_inner &&
        cp.x >= self.rect.pos.x && 
        cp.x <= self.rect.pos.x + self.rect.size.x{
            // bottom edge
            return Some(Edge::Bottom);
        }
        None
    }
}

#[derive(Live, Widget, LiveHook)]
pub struct DesignerContainer {
    #[deref] view: View
}

impl Widget for DesignerContainer {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope){
        self.view.handle_event(cx, event, scope);
    }
                
    fn draw_walk(&mut self, cx: &mut Cx2d, scope:&mut Scope, _walk: Walk) -> DrawStep {
        let data = scope.props.get::<ContainerData>().unwrap();
        // alright lets draw the container, then the child
        let _turtle_step = self.view.turtle_step(id!(inner));
        self.walk = Walk{
            abs_pos: Some(data.rect.pos),
            width: Size::Fixed(data.rect.size.x),
            height: Size::Fixed(data.rect.size.y),
            margin: Default::default()
        };
        while let Some(_next) = self.view.draw(cx, &mut Scope::empty()).step() {
            data.component.draw_all(cx, &mut Scope::empty());
        }
        
       DrawStep::done()
    }
}

#[allow(unused)]
enum FingerMove{
    Pan{start_pan: DVec2},
    DragBody{ptr: LivePtr},
    DragEdge{edge: Edge, rect:Rect, ptr: LivePtr}
}

#[derive(Live, Widget)]
pub struct DesignerView {
    #[walk] walk:Walk,
    #[rust] area:Area,
    #[rust] reapply: bool,
    #[rust(1.5)] zoom: f64,
    #[rust] pan: DVec2,
    #[rust] finger_move: Option<FingerMove>,
    #[live] container: Option<LivePtr>,
    #[live] draw_bg: DrawColor,
    #[rust] containers: ComponentMap<LivePtr, ContainerData>,
    #[redraw] #[rust(DrawList2d::new(cx))] draw_list: DrawList2d,
    #[rust(Pass::new(cx))] pass: Pass,
    #[rust] color_texture: Option<Texture>,
}

impl LiveHook for DesignerView {
    fn after_apply(&mut self, _cx: &mut Cx, _apply: &mut Apply, _index: usize, _nodes: &[LiveNode]){
        
        // hmm. we might need to re-apply the data
        self.reapply = true;
    }
}
        
impl Widget for DesignerView {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope){
        
        match event.hits(cx, self.area) {
            Hit::FingerHoverOver(fh) =>{
                
                // alright so we hover over. lets determine the mouse cursor
                //let corner_inner:f64  = 10.0 * self.zoom;
                //let corner_outer:f64  = 10.0 * self.zoom;
                let mut cursor = None;
                for cd in self.containers.values(){
                    match cd.get_edge(fh.abs -fh.rect.pos, self.zoom, self.pan){
                        Some(edge)=> {
                            cursor = Some(match edge{
                                Edge::Left|Edge::Right=>MouseCursor::EwResize,
                                Edge::Top|Edge::Bottom=>MouseCursor::NsResize,
                            });
                            break;
                        }
                        None=>{}
                    }
                }
                if let Some(cursor) = cursor{
                    cx.set_cursor(cursor);
                }
                else{
                    cx.set_cursor(MouseCursor::Move);
                }
            }
            Hit::FingerHoverOut(_fh)=>{
            }
            Hit::FingerDown(fe) => {
                for (ptr, cd) in self.containers.iter(){
                    match cd.get_edge(fe.abs -fe.rect.pos, self.zoom, self.pan){
                        Some(edge)=>{
                            self.finger_move = Some(FingerMove::DragEdge{
                                rect: cd.rect,
                                ptr: *ptr,
                                edge
                            });
                            break;
                        }
                        None=>()
                    }
                }
                if self.finger_move.is_none(){
                    self.finger_move = Some(FingerMove::Pan{
                        start_pan: self.pan
                    });
                }
            },
            Hit::KeyDown(_k)=>{
            }
            Hit::FingerScroll(fs)=>{
                let last_zoom = self.zoom;
                if fs.scroll.y < 0.0{
                    self.zoom *= 0.9;
                }
                else{
                    self.zoom *= 1.1;
                }
                // we should shift the pan to stay in the same place
                let pan1 = (fs.abs - fs.rect.pos) * last_zoom;
                let pan2 = (fs.abs - fs.rect.pos) * self.zoom;
                // we should keep it in the same place
                
                self.pan += pan1 - pan2;
                
                self.redraw(cx);
            }
            Hit::FingerMove(fe) => {
                match self.finger_move.as_ref().unwrap(){
                    FingerMove::Pan{start_pan} =>{
                        self.pan= *start_pan - (fe.abs - fe.abs_start) * self.zoom;
                        self.redraw(cx);
                    }
                    FingerMove::DragEdge{edge, rect, ptr}=>{
                        let delta = (fe.abs - fe.abs_start)* self.zoom;
                        let r = match edge{
                             Edge::Left=>Rect{
                                pos:dvec2(rect.pos.x + delta.x, rect.pos.y),
                                size:dvec2(rect.size.x - delta.x, rect.size.y)
                             },
                             Edge::Right=>Rect{
                                 pos: rect.pos,
                                 size: dvec2(rect.size.x + delta.x, rect.size.y)
                             },
                             Edge::Top=>Rect{
                                 pos:dvec2(rect.pos.x, rect.pos.y + delta.y),
                                 size:dvec2(rect.size.x, rect.size.y - delta.y)
                             },
                             Edge::Bottom=>Rect{
                                 pos: rect.pos,
                                 size: dvec2(rect.size.x, rect.size.y + delta.y)
                             }
                        };
                        if let Some(container) = self.containers.get_mut(ptr){
                            container.container.redraw(cx);
                            container.rect = r;
                        }
                    }
                    FingerMove::DragBody{ptr:_}=>{
                                                
                    }
                }
            }
            Hit::FingerUp(_) => {
                self.finger_move = None;
            }
            _ => ()
        }
    }
        
    fn draw_walk(&mut self, cx: &mut Cx2d, scope:&mut Scope, walk: Walk) -> DrawStep {
       
        if self.color_texture.is_none(){
            self.color_texture = Some(Texture::new_with_format(
                cx,
                TextureFormat::RenderBGRAu8 {
                    size: TextureSize::Auto,
                },
            ));
            self.pass.add_color_texture(
                cx,
                self.color_texture.as_ref().unwrap(),
                PassClearColor::ClearWith(vec4(0.0, 0.0, 0.0, 0.0)),
            )
        }
        
        if cx.will_redraw(&mut self.draw_list, walk) {
            
            cx.make_child_pass(&self.pass);
            cx.begin_pass(&self.pass, None);
            
            self.draw_list.begin_always(cx);
    
            cx.begin_pass_sized_turtle_no_clip(Layout::flow_down());
            
            let data = scope.props.get::<DesignerData>().unwrap();
            
            // lets draw the component container windows and components
            
            if let Some(selected) = &data.selected{
                if let Some(OutlineNode::Component{ptr,..}) = data.node_map.get(selected){
                    
                    let container_ptr = self.container.unwrap();
                    let cd = self.containers.get_or_insert(cx, *ptr, | cx | {
                        ContainerData{
                            component:WidgetRef::new_from_ptr(cx, Some(*ptr)),
                            container: WidgetRef::new_from_ptr(cx, Some(container_ptr)),
                            rect: rect(50.0,50.0,800.0,600.0)
                        }
                    });
                    
                    if self.reapply{
                        self.reapply = false;
                        cd.container.apply_from_ptr(cx, Some(self.container.unwrap()));
                        cd.component.apply_from_ptr(cx, Some(*ptr));
                    }
                    // ok so we're going to draw the container with the widget inside
                    cd.container.draw_all(cx, &mut Scope::with_props(cd))
                }
            }
            
            cx.end_pass_sized_turtle_no_clip();
            self.draw_list.end(cx);
            cx.end_pass(&self.pass);
        }
        
        self.draw_bg.draw_vars.set_texture(0, self.color_texture.as_ref().unwrap());
        let rect = cx.walk_turtle_with_area(&mut self.area, walk);
        self.draw_bg.draw_abs(cx, rect);
            
        cx.set_pass_area_with_origin(
            &self.pass,
            self.area,
            dvec2(0.0,0.0)
        );
        cx.set_pass_shift_scale(&self.pass, self.pan, dvec2(self.zoom,self.zoom));
        
        DrawStep::done()
    }
}