
use {
    crate::{
        app::{AppData},
        makepad_widgets::*,
        makepad_widgets::file_tree::FileTree,
    },
};

live_design!{
    use link::widgets::*;
        
    pub StudioFileTree = {{StudioFileTree}}{
        file_tree: <FileTree>{}
    }
}
 
#[derive(Live, LiveHook, Widget)] 
pub struct StudioFileTree{
    #[wrap] #[live] pub file_tree: FileTree
}

impl Widget for StudioFileTree {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope:&mut Scope, walk:Walk)->DrawStep{
        while self.file_tree.draw_walk(cx, scope, walk).is_step() {
            self.file_tree.set_folder_is_open(cx, live_id!(root).into(), true, Animate::No);
            scope.data.get_mut::<AppData>().unwrap().file_system.draw_file_node(
                cx,
                live_id!(root).into(),
                0,
                &mut self.file_tree
            );
        }
        DrawStep::done()
    }
    
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope){
        self.file_tree.handle_event(cx, event, scope);
    }
}