
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
    pub Root = <RootBase> { design_window = <Designer> {} }
}

#[derive(Live, LiveRegisterWidget, WidgetRef)]
pub struct Root {
    #[rust] draw_state: DrawStateWrap<DrawState>,
    #[rust] windows: ComponentMap<LiveId, WidgetRef>,
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
                    return self.windows.get_or_insert(cx, id, | cx | {WidgetRef::new(cx)})
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

#[derive(Clone)]
enum DrawState {
    Window(usize),
}

impl WidgetNode for Root{
    fn redraw(&mut self, cx: &mut Cx) {
        for window in self.windows.values_mut() {
            window.redraw(cx);
        }
    }
    
    fn area(&self)->Area{Area::Empty}
    
    fn walk(&mut self, _cx:&mut Cx) -> Walk {Walk::default()}
        
    fn find_widgets(&self, path: &[LiveId], cached: WidgetCache, results:&mut WidgetSet){
        for window in self.windows.values() {
            window.find_widgets(path, cached, results);
        }
    }
    
    fn uid_to_widget(&self, uid:WidgetUid)->WidgetRef{
        for window in self.windows.values() {
            let x = window.uid_to_widget(uid);
            if !x.is_empty(){return x}
        }
        WidgetRef::empty()
    }
        
}

impl Widget for Root {
    
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        for window in self.windows.values_mut() {
            window.handle_event(cx, event, scope);
        }
    }
    
     fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, _walk: Walk) -> DrawStep {
        // check if we are in a 3D mode. Ifso we wrap Cx2d in Cx3d
        // and call into the next layer as draw_3d
        // the window then continues back into the 2d draw context
        
        self.draw_state.begin(cx, DrawState::Window(0));
        
        while let Some(DrawState::Window(step)) = self.draw_state.get() {
            
            if let Some(window) = self.windows.values_mut().nth(step){
                let walk = window.walk(cx);
                window.draw_walk(cx, scope, walk)?; 
                self.draw_state.set(DrawState::Window(step+1));
            }
            else{
                self.draw_state.end();
            }
        }
        DrawStep::done()
    }
}