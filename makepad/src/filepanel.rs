// lets classify the types of things we can autogenerate
// empty UI file
// View entry
// Quad entry
// QuadwShader entry

use render::*;
use widget::*;
use crate::filetree::*;

#[derive(Clone)]
pub struct FilePanel {
    pub file_tree: FileTree,
    pub new_file_btn: NormalButton,
} 

#[derive(Clone, PartialEq)]
pub enum FilePanelEvent {
    NewFile {name: String, template: String},
    Cancel,
    None,
}

impl FilePanel {
    pub fn proto(cx: &mut Cx) -> Self {
        Self {
            file_tree: FileTree::proto(cx),
            new_file_btn: NormalButton::proto(cx),
        }
    }
    
    pub fn handle_file_panel(&mut self, cx: &mut Cx, event: &mut Event) -> FileTreeEvent {
        //self.new_file_btn.handle_button(cx, event);
        self.file_tree.handle_file_tree(cx, event)
    }
    
    pub fn draw_tab_buttons(&mut self, _cx: &mut Cx){
        //self.new_file_btn.draw_button(cx, "HELLO");
    }
    
    pub fn draw_file_panel(&mut self, cx: &mut Cx) {
        self.file_tree.draw_file_tree(cx)
    }
}