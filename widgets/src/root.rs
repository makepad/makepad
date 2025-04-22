
use {
    crate::{
        widget::*,
        makepad_derive_widget::*,
        makepad_draw::*,
    }
};

live_design!{
    link widgets;
    
    use link::widgets::*;
    use link::designer::Designer;
    
    pub RootBase = {{Root}} {}
    pub Root = <RootBase> {
        design_window = <Designer> {}
        xr_hands = <XrHands>{}
    }
}

#[derive(Live, LiveRegisterWidget, WidgetRef)]
pub struct Root {
    #[rust] components: ComponentMap<LiveId, WidgetRef>,
    #[rust(DrawList::new(cx))] xr_draw_list: DrawList,
    #[live] xr_pass: Pass,
}
 
impl LiveHook for Root {
    fn apply_value_instance(&mut self, cx: &mut Cx, apply: &mut Apply, index: usize, nodes: &[LiveNode]) -> usize {
        let id = nodes[index].id;
        match apply.from {
            ApplyFrom::NewFromDoc {..} | ApplyFrom::UpdateFromDoc {..} => {
                if nodes[index].origin.has_prop_type(LivePropType::Instance) {
                    // only open the design window 
                    if id == live_id!(design_window) && !cx.in_makepad_studio(){
                        return nodes.skip_node(index);
                    }
                    if id == live_id!(xr_hands) && !cx.os_type().has_xr_mode(){
                        return nodes.skip_node(index);
                    }
                    return self.components.get_or_insert(cx, id, | cx | {WidgetRef::new(cx)})
                        .apply(cx, apply, index, nodes);
                }
                else {
                    cx.apply_error_no_matching_field(live_error_origin!(), index, nodes);
                }
            }
            _ => ()
        }
        nodes.skip_node(index)
    }
}


impl WidgetNode for Root{
    fn redraw(&mut self, cx: &mut Cx) {
        for component in self.components.values_mut() {
            component.redraw(cx);
        }
    }
    
    fn area(&self)->Area{Area::Empty}
    
    fn walk(&mut self, _cx:&mut Cx) -> Walk {Walk::default()}
        
    fn find_widgets(&self, path: &[LiveId], cached: WidgetCache, results:&mut WidgetSet){
        for component in self.components.values() {
            component.find_widgets(path, cached, results);
        }
    }
    
    fn uid_to_widget(&self, uid:WidgetUid)->WidgetRef{
        for component in self.components.values() {
            let x = component.uid_to_widget(uid);
            if !x.is_empty(){return x}
        }
        WidgetRef::empty()
    }
        
}

impl Widget for Root {
    
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if let Event::Draw(e) = event {
            if cx.in_xr_mode(){
                if  !e.xr_state.is_some(){
                    return
                }
                let mut cx_draw = CxDraw::new(cx, e);
                let cx = &mut Cx3d::new(&mut cx_draw);
                // lets begin a 3D drawlist in the global context
                self.xr_pass.set_as_xr_pass(cx);
                cx.begin_pass(&self.xr_pass, Some(4.0));
                self.xr_draw_list.begin_always(cx);
                self.draw_3d_all(cx, scope);
                self.xr_draw_list.end(cx);
                cx.end_pass(&self.xr_pass);
                return
            }
            else{
                let mut cx_draw = CxDraw::new(cx, e);
                let cx = &mut Cx2d::new(&mut cx_draw);
                self.draw_all(cx, scope);
                return
            }
        }
        
        for component in self.components.values_mut() {
            component.handle_event(cx, event, scope);
        }
    }
    
    fn draw_3d(&mut self, cx: &mut Cx3d, scope:&mut Scope)->DrawStep{
        for component in self.components.values(){
            component.draw_3d_all(cx, scope);
        }
        DrawStep::done()
    }
    
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        for component in self.components.values(){
            component.draw_walk_all(cx, scope, walk);
        }
        DrawStep::done()
    }
}