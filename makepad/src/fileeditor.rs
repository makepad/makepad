//use syn::Type;
use makepad_render::*;
use makepad_widget::*;

use crate::jseditor::*;
use crate::rusteditor::*;
use crate::plaineditor::*;
use crate::searchindex::*;
use crate::appstorage::*;

use std::collections::HashMap;

#[derive(Clone)]
pub struct FileEditors {
    pub rust_editor: RustEditor,
    pub js_editor: JSEditor,
    pub plain_editor: PlainEditor,
    pub editors: HashMap<u64, FileEditor>
    //text_editor: TextEditor
}

#[derive(Clone)]
pub enum FileEditor {
    Rust(RustEditor),
    JS(JSEditor),
    Plain(PlainEditor)
    //Text(TextEditor)
}

impl FileEditor {
    pub fn handle_file_editor(&mut self, cx: &mut Cx, event: &mut Event, atb: &mut AppTextBuffer, search_index: Option<&mut SearchIndex>) -> TextEditorEvent {
        match self {
            FileEditor::Rust(re) => re.handle_rust_editor(cx, event, atb, search_index),
            FileEditor::JS(re) => re.handle_js_editor(cx, event, atb),
            FileEditor::Plain(re) => re.handle_plain_editor(cx, event, atb),
        }
    }
    
    pub fn set_last_cursor(&mut self, cx: &mut Cx, cursor:(usize, usize), at_top:bool) {
        match self {
            FileEditor::Rust(re) => re.text_editor.set_last_cursor(cx, cursor, at_top),
            FileEditor::JS(re) => re.text_editor.set_last_cursor(cx, cursor, at_top),
            FileEditor::Plain(re) => re.text_editor.set_last_cursor(cx, cursor, at_top),
        }
    }
    
    pub fn set_key_focus(&mut self, cx: &mut Cx) {
        match self {
            FileEditor::Rust(re) => re.text_editor.set_key_focus(cx),
            FileEditor::JS(re) => re.text_editor.set_key_focus(cx),
            FileEditor::Plain(re) => re.text_editor.set_key_focus(cx),
        }
    }
    
    pub fn has_key_focus(&mut self, cx: &mut Cx)->bool {
        match self {
            FileEditor::Rust(re) => re.text_editor.has_key_focus(cx),
            FileEditor::JS(re) => re.text_editor.has_key_focus(cx),
            FileEditor::Plain(re) => re.text_editor.has_key_focus(cx),
        }
    }
    
    pub fn get_scroll_pos(&mut self, cx: &mut Cx) -> Vec2 {
        match self {
            FileEditor::Rust(re) => re.text_editor.view.get_scroll_pos(cx),
            FileEditor::JS(re) => re.text_editor.view.get_scroll_pos(cx),
            FileEditor::Plain(re) => re.text_editor.view.get_scroll_pos(cx),
        }
    }
    
    pub fn set_scroll_pos_on_load(&mut self, pos: Vec2) {
        match self {
            FileEditor::Rust(re) => re.text_editor._scroll_pos_on_load = Some(pos),
            FileEditor::JS(re) => re.text_editor._scroll_pos_on_load = Some(pos),
            FileEditor::Plain(re) => re.text_editor._scroll_pos_on_load = Some(pos),
        }
    }
    
    pub fn draw_file_editor(&mut self, cx: &mut Cx, atb: &mut AppTextBuffer, search_index: &mut SearchIndex) {
        match self {
            FileEditor::Rust(re) => re.draw_rust_editor(cx, atb, Some(search_index)),
            FileEditor::JS(re) => re.draw_js_editor(cx, atb, Some(search_index)),
            FileEditor::Plain(re) => re.draw_plain_editor(cx, atb, Some(search_index)),
        }
    }
    
    pub fn update_token_chunks(path: &str, atb: &mut AppTextBuffer, search_index: &mut SearchIndex) {
        // check which file extension we have to spawn a new editor
        if path.ends_with(".rs") || path.ends_with(".toml") || path.ends_with(".ron") {
            RustTokenizer::update_token_chunks(atb, Some(search_index));
        }
        else if path.ends_with(".js") || path.ends_with(".html") {
            JSTokenizer::update_token_chunks(atb, Some(search_index));
        }
        else {
            PlainTokenizer::update_token_chunks(&mut atb.text_buffer, Some(search_index));
        }
    }
}

impl FileEditors {
    pub fn does_path_match_editor_type(&mut self, path: &str, editor_id:u64)->bool{
        if !self.editors.contains_key(&editor_id){
            return false;
        }
        match self.editors.get(&editor_id).unwrap(){
            FileEditor::Rust(_) =>path.ends_with(".rs") || path.ends_with(".toml") || path.ends_with(".ron"),
            FileEditor::JS(_) => path.ends_with(".js") || path.ends_with(".html"),
            FileEditor::Plain(_) => !(path.ends_with(".rs") || path.ends_with(".toml") || path.ends_with(".ron") || path.ends_with(".js") || path.ends_with(".html"))
        }
    }
    
    pub fn get_file_editor_for_path(&mut self, path: &str, editor_id:u64) -> (&mut FileEditor, bool) {
        
        // check which file extension we have to spawn a new editor
        let is_new = !self.editors.contains_key(&editor_id);
        if is_new {
            let editor = if path.ends_with(".rs") || path.ends_with(".toml") || path.ends_with(".ron") {
                FileEditor::Rust(RustEditor {
                    ..self.rust_editor.clone()
                })
            }
            else if path.ends_with(".js") || path.ends_with(".html") {
                FileEditor::JS(JSEditor {
                    ..self.js_editor.clone()
                })
            }
            else {
                FileEditor::Plain(PlainEditor {
                    ..self.plain_editor.clone()
                })
            };
            self.editors.insert(editor_id, editor);
        }
        (self.editors.get_mut(&editor_id).unwrap(), is_new)
    }
    
    pub fn highest_file_editor_id(&self) -> u64 {
        let mut max_id = 0;
        for (id, _) in &self.editors {
            if *id > max_id {
                max_id = *id;
            }
        }
        max_id
    }
}

pub fn path_file_name(path: &str) -> String {
    if let Some(pos) = path.rfind('/') {
        path[pos + 1..path.len()].to_string()
    }
    else {
        path.to_string()
    }
}
