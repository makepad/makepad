
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
 
#[derive(Live, Widget)] 
pub struct StudioFileTree{
    #[wrap] #[live] pub file_tree: FileTree
}
impl LiveHook for StudioFileTree{
    fn after_new_from_doc(&mut self, cx:&mut Cx){
        self.file_tree.set_folder_is_open(cx, live_id!(makepad).into(), true, Animate::No);
    }
}

impl Widget for StudioFileTree {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope:&mut Scope, walk:Walk)->DrawStep{
        while self.file_tree.draw_walk(cx, scope, walk).is_step() {
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

impl StudioFileTreeRef{
    pub fn set_folder_is_open(
        &self,
        cx: &mut Cx,
        node_id: LiveId,
        is_open: bool,
        animate: Animate,
    ) {
        if let Some(mut inner) = self.borrow_mut(){
            inner.file_tree.set_folder_is_open(cx, node_id, is_open, animate);
        }
    }
}