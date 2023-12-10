
use {
    crate::{
        app::{StudioData},
        makepad_widgets::*,
        makepad_widgets::file_tree::FileTree,
    },
    std::{
        env,
    },
};

live_design!{
    import makepad_widgets::theme_desktop_dark::*;
        
    StudioFileTree = {{StudioFileTree}}{
        file_tree: <FileTree>{}
    }
} 
 
#[derive(Live)] 
pub struct StudioFileTree{
    #[live] pub file_tree: FileTree
} 
 
impl LiveHook for StudioFileTree{
    fn before_live_design(cx:&mut Cx){
        register_widget!(cx, StudioFileTree)
    }
}

impl Widget for StudioFileTree {
    fn redraw(&mut self, cx: &mut Cx) {
        self.file_tree.redraw(cx);
    }
    
    fn walk(&mut self, cx:&mut Cx) -> Walk {
        self.file_tree.walk(cx)
    }
    
    fn draw_walk(&mut self, cx: &mut Cx2d, scope:&mut WidgetScope, walk:Walk)->WidgetDraw{
        while let Some(_) = self.file_tree.draw_walk(cx, scope, walk).hook_widget() {
            self.file_tree.set_folder_is_open(cx, live_id!(root).into(), true, Animate::No);
             scope.data.get_mut::<StudioData>().file_system.draw_file_node(
                cx,
                live_id!(root).into(),
                &mut self.file_tree
            );
        }
        WidgetDraw::done()
    }
    
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut WidgetScope){
        self.file_tree.handle_event(cx, event, scope);
    }
}
#[derive(Clone, Debug, PartialEq, WidgetRef)]
pub struct StudioFileTreeRef(WidgetRef);