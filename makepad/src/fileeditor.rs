//use syn::Type;
use render::*;
use editor::*;

#[derive(Clone)]
pub struct FileEditorTemplates {
    pub rust_editor: RustEditor,
    pub js_editor: JSEditor,
    pub plain_editor: PlainEditor
    //text_editor: TextEditor
}

#[derive(Clone)]
pub enum FileEditor {
    Rust(RustEditor),
    JS(JSEditor),
    Plain(PlainEditor)
    //Text(TextEditor)
}

#[derive(Clone)]
pub enum FileEditorEvent {
    None,
    LagChange,
    Change
}

fn code_editor_to_file_editor(event: CodeEditorEvent)->FileEditorEvent {
    match event {
        CodeEditorEvent::Change => FileEditorEvent::Change,
        CodeEditorEvent::LagChange => FileEditorEvent::LagChange,
        _ => FileEditorEvent::None
    }
}

impl FileEditor {
    pub fn handle_file_editor(&mut self, cx: &mut Cx, event: &mut Event, text_buffer: &mut TextBuffer) -> FileEditorEvent {
        match self {
            FileEditor::Rust(re) => code_editor_to_file_editor(re.handle_rust_editor(cx, event, text_buffer)),
            FileEditor::JS(re) => code_editor_to_file_editor(re.handle_js_editor(cx, event, text_buffer)),
            FileEditor::Plain(re) => code_editor_to_file_editor(re.handle_plain_editor(cx, event, text_buffer)),
        }
    }
    
    pub fn set_key_focus(&mut self, cx: &mut Cx) {
        match self {
            FileEditor::Rust(re) => re.code_editor.set_key_focus(cx),
            FileEditor::JS(re) => re.code_editor.set_key_focus(cx),
            FileEditor::Plain(re) => re.code_editor.set_key_focus(cx),
        }
    }

    pub fn get_scroll_pos(&mut self, cx: &mut Cx)->Vec2{
        match self {
            FileEditor::Rust(re) => re.code_editor.view.get_scroll_pos(cx),
            FileEditor::JS(re) => re.code_editor.view.get_scroll_pos(cx),
            FileEditor::Plain(re) => re.code_editor.view.get_scroll_pos(cx),
        }
    }
    
    pub fn set_scroll_pos_on_load(&mut self, pos:Vec2){
        match self {
            FileEditor::Rust(re) => re.code_editor._scroll_pos_on_load = Some(pos),
            FileEditor::JS(re) => re.code_editor._scroll_pos_on_load = Some(pos),
            FileEditor::Plain(re) => re.code_editor._scroll_pos_on_load = Some(pos),
        }
    }

    pub fn draw_file_editor(&mut self, cx: &mut Cx, text_buffer: &mut TextBuffer) {
        match self {
            FileEditor::Rust(re) => re.draw_rust_editor(cx, text_buffer),
            FileEditor::JS(re) => re.draw_js_editor(cx, text_buffer),
            FileEditor::Plain(re) => re.draw_plain_editor(cx, text_buffer),
        }
    }
    
    pub fn create_file_editor_for_path(path: &str, template: &FileEditorTemplates) -> FileEditor {
        // check which file extension we have to spawn a new editor
        if path.ends_with(".rs") || path.ends_with(".toml")  || path.ends_with(".ron"){
            FileEditor::Rust(RustEditor {
                ..template.rust_editor.clone()
            })
        }
        else if path.ends_with(".js") || path.ends_with(".html"){
            FileEditor::JS(JSEditor {
                ..template.js_editor.clone()
            })
        }
        else {
            FileEditor::Plain(PlainEditor {
                ..template.plain_editor.clone()
            })
        }
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
