use {
    self::super::open_ai_data::*,
    self::super::google_ai_data::*,
    crate::{
        app::AppAction,
        file_system::file_system::{FileSystem,OpenDocument},
        makepad_widgets::*,
        makepad_micro_serde::*
    },
};

pub struct AiChatManager{
    pub projects: Vec<AiProject>,
    pub models: Vec<AiModel>,
    pub contexts: Vec<BaseContext>,
}
const OPENAI_DEFAULT_URL: &'static str = "https://api.openai.com/v1/chat/completions";

impl Default for AiChatManager{
    fn default()->Self{
        Self{
            models: vec![
                AiModel{
                    name: "local".to_string(),
                    backend: AiBackend::OpenAI{
                        url:"http://10.0.0.113:8080/v1/chat/completions".to_string(),
                        model:"".to_string(),
                        reasoning_effort: None,
                        key:"".to_string()
                    }
                },
                AiModel{
                    name: "gemini 2.0 pro".to_string(),
                    backend: AiBackend::Google{
                        url:"https://generativelanguage.googleapis.com/v1beta/models/gemini-2.0-pro-exp-02-05:streamGenerateContent?alt=sse&key=".to_string(),
                        key:std::fs::read_to_string("GOOGLE_KEY").unwrap_or("".to_string())
                    }
                },
                AiModel{
                    name:"o3-mini".to_string(),
                    backend: AiBackend::OpenAI{
                        url: OPENAI_DEFAULT_URL.to_string(),
                        model: "o3-mini".to_string(),
                        reasoning_effort: Some("high".to_string()),
                        key: std::fs::read_to_string("OPENAI_KEY").unwrap_or("".to_string())
                    },
                },
                AiModel{
                    name: "gpt-4o".to_string(),
                    backend: AiBackend::OpenAI{
                        url: OPENAI_DEFAULT_URL.to_string(),
                        reasoning_effort: None,
                        model: "gpt-4o".to_string(),
                        key: std::fs::read_to_string("OPENAI_KEY").unwrap_or("".to_string())
                    },
                },
            ],
            contexts: vec![
                BaseContext{
                    name: "Chat".to_string(),
                    apply: AiApply::PatchDSL,
                    system_pre: live_id!(CHAT_PRE),
                    system_post: live_id!(CHAT_POST),
                    general_post: live_id!(CHAT_GENERAL),
                    files: vec![]
                },
                BaseContext{
                    name: "Rust".to_string(),
                    apply: AiApply::PatchDSL,
                    system_pre: live_id!(RUST_PRE),
                    system_post: live_id!(RUST_POST),
                    general_post: live_id!(RUST_GENERAL),
                    files: vec![]
                },
                BaseContext{
                    name: "Next".to_string(),
                    apply: AiApply::WholeFile,
                    system_pre: live_id!(NEXT_PRE),
                    system_post: live_id!(NEXT_POST),
                    general_post: live_id!(NEXT_GENERAL),
                    files: vec![]
                },
                BaseContext{
                    name: "Next All".to_string(),
                    apply: AiApply::WholeFile,
                    system_pre: live_id!(NEXT_ALL_PRE),
                    system_post: live_id!(NEXT_ALL_POST),
                    general_post: live_id!(NEXT_ALL_GENERAL),
                    files: vec![]
                },
                BaseContext{
                    name: "Next UI".to_string(),
                    apply: AiApply::PatchDSL,
                    system_pre: live_id!(FNEXT_UI_PRE),
                    system_post: live_id!(NEXT_UI_POST),
                    general_post: live_id!(NEXT_UI_GENERAL),
                    files: vec![]
                },
                BaseContext{
                    name: "Makepad All".to_string(),
                    apply: AiApply::WholeFile,
                    system_pre: live_id!(ALL_PRE),
                    system_post: live_id!(ALL_POST),
                    general_post: live_id!(GENERAL_POST),
                    files: vec![
                        AiContextFile::new("Example code","examples/ai_docs/src/app.rs"),
                        AiContextFile::new("Example code","examples/ai_docs/src/app2.rs"),
                        AiContextFile::new("Example code","examples/ai_docs/src/app3.rs"),
                        AiContextFile::new("Example code","examples/ai_docs/src/app4.rs"),
                    ]
                },
                BaseContext{
                    name: "Makepad UI".to_string(),
                    apply: AiApply::PatchDSL,
                    system_pre: live_id!(UI_PRE),
                    system_post: live_id!(UI_POST),
                    general_post: live_id!(GENERAL_POST),
                    files: vec![
                        AiContextFile::new("Example code","examples/ai_docs/src/app.rs"),
                        AiContextFile::new("Example code","examples/ai_docs/src/app2.rs"),
                        AiContextFile::new("Example code","examples/ai_docs/src/app3.rs"),
                        AiContextFile::new("Example code","examples/ai_docs/src/app4.rs"),
                    ]
                },
                BaseContext{
                    name: "Makepad Internal".to_string(),
                    apply: AiApply::WholeFile,
                    system_pre: live_id!(INTERNAL_PRE),
                    system_post: live_id!(INTERNAL_POST),
                    general_post: live_id!(INTERNAL_GENERAL),
                    files: vec![
                        AiContextFile::new("Example code","examples/ai_docs/src/app.rs"),
                        AiContextFile::new("Example code","examples/ai_docs/src/app2.rs"),
                        AiContextFile::new("Example code","examples/ai_docs/src/app3.rs"),
                        AiContextFile::new("Example code","examples/ai_docs/src/app4.rs"),                        
                        AiContextFile::new("","platform/live_compiler/src/live_document.rs"),
                        AiContextFile::new("","platform/live_compiler/src/live_error.rs"),
                        AiContextFile::new("","platform/live_compiler/src/live_eval.rs"),
                        AiContextFile::new("","platform/live_compiler/src/live_expander.rs"),
                        AiContextFile::new("","platform/live_compiler/src/live_node.rs"),
                        AiContextFile::new("","platform/live_compiler/src/live_node_vec.rs"),
                        AiContextFile::new("","platform/live_compiler/src/live_parser.rs"),
                        AiContextFile::new("","platform/live_compiler/src/live_ptr.rs"),
                        AiContextFile::new("","platform/live_compiler/src/live_registry.rs"),
                        AiContextFile::new("","platform/live_compiler/src/live_token.rs"),
                        AiContextFile::new("","platform/live_compiler/src/span.rs"),
                        AiContextFile::new("","platform/live_compiler/src/util.rs"),
                        
                        AiContextFile::new("","platform/live_tokenizer/src/char_ext.rs"),
                        AiContextFile::new("","platform/live_tokenizer/src/colorhex.rs"),
                        AiContextFile::new("","platform/live_tokenizer/src/full_token.rs"),
                        AiContextFile::new("","platform/live_tokenizer/src/live_error_origin.rs"),
                        AiContextFile::new("","platform/live_tokenizer/src/tokenizer.rs"),
                        AiContextFile::new("","platform/live_tokenizer/src/vec4_ext.rs"),
                        
                        AiContextFile::new("","platform/shader_compiler/src/analyse.rs"),
                        AiContextFile::new("","platform/shader_compiler/src/builtin.rs"),
                        AiContextFile::new("","platform/shader_compiler/src/const_eval.rs"),
                        AiContextFile::new("","platform/shader_compiler/src/const_gather.rs"),
                        AiContextFile::new("","platform/shader_compiler/src/dep_analyse.rs"),
                        AiContextFile::new("","platform/shader_compiler/src/generate.rs"),
                        AiContextFile::new("","platform/shader_compiler/src/generate_glsl.rs"),
                        AiContextFile::new("","platform/shader_compiler/src/generate_hlsl.rs"),
                        AiContextFile::new("","platform/shader_compiler/src/generate_metal.rs"),
                        AiContextFile::new("","platform/shader_compiler/src/lhs_check.rs"),
                        AiContextFile::new("","platform/shader_compiler/src/shader_ast.rs"),
                        AiContextFile::new("","platform/shader_compiler/src/shader_parser.rs"),
                        AiContextFile::new("","platform/shader_compiler/src/shader_registry.rs"),
                        AiContextFile::new("","platform/shader_compiler/src/swizzle.rs"),
                        AiContextFile::new("","platform/shader_compiler/src/ty_check.rs"),
                        AiContextFile::new("","platform/shader_compiler/src/util.rs"),
                        
                        AiContextFile::new("","platform/src/event/designer.rs"),
                        AiContextFile::new("","platform/src/event/drag_drop.rs"),
                        AiContextFile::new("","platform/src/event/event.rs"),
                        AiContextFile::new("","platform/src/event/finger.rs"),
                        AiContextFile::new("","platform/src/event/keyboard.rs"),
                        AiContextFile::new("","platform/src/event/network.rs"),
                        AiContextFile::new("","platform/src/event/video_playback.rs"),
                        AiContextFile::new("","platform/src/event/window.rs"),
                                                                        
                        AiContextFile::new("","platform/src/action.rs"),
                        AiContextFile::new("","platform/src/animator.rs"),
                        AiContextFile::new("","platform/src/app_main.rs"),
                        AiContextFile::new("","platform/src/area.rs"),
                        AiContextFile::new("","platform/src/cx.rs"),
                        AiContextFile::new("","platform/src/cx_api.rs"),
                        AiContextFile::new("","platform/src/debug.rs"),
                        
                        AiContextFile::new("","platform/src/draw_list.rs"),
                        AiContextFile::new("","platform/src/draw_shader.rs"),
                        AiContextFile::new("","platform/src/draw_vars.rs"),
                        
                        AiContextFile::new("","platform/src/file_dialogs.rs"),
                        AiContextFile::new("","platform/src/geometry.rs"),
                        AiContextFile::new("","platform/src/gpu_info.rs"),
                        AiContextFile::new("","platform/src/id_pool.rs"),
                        AiContextFile::new("","platform/src/live_atomic.rs"),
                        AiContextFile::new("","platform/src/live_cx.rs"),
                        AiContextFile::new("","platform/src/live_prims.rs"),
                        AiContextFile::new("","platform/src/live_traits.rs"),
                        AiContextFile::new("","platform/src/log.rs"),
                        AiContextFile::new("","platform/src/pass.rs"),
                        AiContextFile::new("","platform/src/scope.rs"),
                        AiContextFile::new("","platform/src/studio.rs"),
                        AiContextFile::new("","platform/src/texture.rs"),
                        AiContextFile::new("","platform/src/thread.rs"),
                        AiContextFile::new("","platform/src/ui_runner.rs"),
                        AiContextFile::new("","platform/src/video.rs"),
                        AiContextFile::new("","platform/src/web_socket.rs"),
                        AiContextFile::new("","platform/src/window.rs"),
                        
                        AiContextFile::new("","draw/src/cx_2d.rs"),
                        AiContextFile::new("","draw/src/draw_list_2d.rs"),
                        
                        AiContextFile::new("","draw/src/match_event.rs"),
                        AiContextFile::new("","draw/src/nav.rs"),
                        AiContextFile::new("","draw/src/overlay.rs"),
                        AiContextFile::new("","draw/src/shader/draw_color.rs"),
                        AiContextFile::new("","draw/src/shader/draw_quad.rs"),
                        AiContextFile::new("","draw/src/shader/draw_text.rs"),
                        AiContextFile::new("","draw/src/turtle.rs"),
                        AiContextFile::new("","widgets/src/button.rs"),
                        AiContextFile::new("","widgets/src/check_box.rs"),
                        AiContextFile::new("","widgets/src/dock.rs"),
                        AiContextFile::new("","widgets/src/drop_down.rs"),
                        AiContextFile::new("","widgets/src/expandable_panel.rs"),
                        AiContextFile::new("","widgets/src/file_tree.rs"),
                        AiContextFile::new("","widgets/src/flat_list.rs"),
                        AiContextFile::new("","widgets/src/portal_list.rs"),
                        AiContextFile::new("","widgets/src/html.rs"),
                        AiContextFile::new("","widgets/src/image.rs"),
                        AiContextFile::new("","widgets/src/image_cache.rs"),
                        AiContextFile::new("","widgets/src/label.rs"),
                        AiContextFile::new("","widgets/src/link_label.rs"),
                        AiContextFile::new("","widgets/src/markdown.rs"),
                        AiContextFile::new("","widgets/src/portal_list.rs"),
                        AiContextFile::new("","widgets/src/radio_button.rs"),
                        AiContextFile::new("","widgets/src/scroll_bar.rs"),
                        AiContextFile::new("","widgets/src/scroll_bars.rs"),
                        AiContextFile::new("","widgets/src/slider.rs"),
                        AiContextFile::new("","widgets/src/slides_view.rs"),
                        AiContextFile::new("","widgets/src/tab.rs"),
                        AiContextFile::new("","widgets/src/tab_bar.rs"),
                        AiContextFile::new("","widgets/src/tab_close_button.rs"),
                        AiContextFile::new("","widgets/src/text_flow.rs"),
                        AiContextFile::new("","widgets/src/text_input.rs"),
                        AiContextFile::new("","widgets/src/theme_desktop_dark.rs"),
                        AiContextFile::new("","widgets/src/view.rs"),
                        AiContextFile::new("","widgets/src/view_ui.rs"),
                        AiContextFile::new("","widgets/src/widget.rs"),
                        AiContextFile::new("","widgets/src/widget_match_event.rs"),
                        AiContextFile::new("","widgets/src/window.rs"),
                        AiContextFile::new("","widgets/src/designer.rs"),
                        AiContextFile::new("","widgets/src/designer_data.rs"),
                        AiContextFile::new("","widgets/src/designer_dummy.rs"),
                        AiContextFile::new("","widgets/src/designer_outline.rs"),
                        AiContextFile::new("","widgets/src/designer_outline_tree.rs"),
                        AiContextFile::new("","widgets/src/designer_theme.rs"),
                        AiContextFile::new("","widgets/src/designer_toolbox.rs"),
                        AiContextFile::new("","widgets/src/designer_view.rs"),
                    ],
                },
            ],
            projects: vec![
                AiProject{
                    name:"None".to_string(),
                    files:vec![]
                },
                AiProject{
                    name:"makepad-example-simple".to_string(),
                    files:vec![
                        AiContextFile::new("Main app to rewrite","examples/simple/src/app.rs")
                    ]
                },
                AiProject{
                    name:"makepad-portal-list".to_string(),
                    files:vec![
                        AiContextFile::new("","widgets/src/portal_list2.rs")
                    ]
                }
            ]
        }
    }
}

pub struct AiProject{
    pub name:String,
    pub files: Vec<AiContextFile>
}

#[derive(Debug, SerRon, DeRon)]
pub enum AiApply{
    PatchDSL,
    WholeFile,
    None
}
    
#[derive(Debug, SerRon, DeRon)]
pub struct AiModel{
    pub name: String,
    pub backend: AiBackend
}

#[derive(Clone, Debug, SerRon, DeRon)]
pub enum AiBackend{
    Google{
        url:String, 
        key: String,
    },
    OpenAI{
        url:String, 
        model:String,
        reasoning_effort: Option<String>,
        key: String,
    }
}

pub struct AiContextFile{
    kind: String,
    path: String
}
impl AiContextFile{
    fn new(kind:&str, path:&str)->Self{
        Self{kind:kind.to_string(), path:path.to_string()}
    }
}

pub struct BaseContext{
    pub name: String,
    pub apply: AiApply,
    pub system_pre: LiveId,
    pub system_post: LiveId,
    pub general_post: LiveId,
    pub files: Vec<AiContextFile>
}

#[derive(Debug, SerRon, DeRon, Clone)]
pub enum AiContext{
    Snippet{name:String, language:String, content:String},
    File{file_id:LiveId}
}

#[derive(Default, Debug, SerRon, DeRon, Clone)]
pub struct AiUserMessage{
    pub auto_run: bool,
    pub model: String,
    pub project: String,
    pub base_context: String,
    pub context: Vec<AiContext>,
    pub message:String
}

#[derive(Debug, SerRon, DeRon, Clone)]
pub enum AiChatMessage{
    User(AiUserMessage),
    Assistant(String)
}


#[derive(Debug, SerRon, DeRon, Clone)]
pub struct AiChatMessages{
    pub last_time: f64,
    pub messages: Vec<AiChatMessage>
}

impl AiChatMessages{
    fn new()->Self{
        AiChatMessages{
            last_time: 0.0,
            messages: vec![AiChatMessage::User(AiUserMessage::default())],
        }
    }
    
    fn follow_up(&mut self){
        if let Some(AiChatMessage::User(usr)) = self.messages.iter().rev().nth(1).cloned(){
            let next = if usr.base_context == "Makepad UI"{
                "Next UI"
            }
            else if usr.base_context == "Makepad All"{
                "Next All"
            }
            else{
                "Next"
            };
            self.messages.push(AiChatMessage::User(AiUserMessage{
                auto_run: usr.auto_run,
                model: usr.model.clone(),
                project: usr.project.clone(),
                base_context: next.to_string(),
                context: vec![],
                message:"".to_string()
            }));
        }
    }
}

#[derive(Debug, SerRon, DeRon)]
pub struct AiChatFile{
    pub history: Vec<AiChatMessages>,
}

#[derive(Debug)]
pub struct AiInFlight{
    request_id: LiveId,
    backend: AiBackend,
    history_slot: usize
}

#[derive(Debug)]
pub struct AiChatDocument{
    pub in_flight: Option<AiInFlight>,
    pub auto_run: bool,
    pub file: AiChatFile
}

impl AiChatDocument{
    pub fn load_or_empty(data: &str)->AiChatDocument{
        match AiChatFile::deserialize_ron(data).map_err(|e| format!("{:?}", e)){
            Err(e)=>{
                error!("Error parsing AiChatDocument {e}");
                Self{
                    auto_run: true,
                    in_flight: None,
                    file: AiChatFile::new()
                }
            }
            Ok(file)=>{
                Self{
                    auto_run: true,
                    in_flight: None,
                    file
                }
            }
        }
    }
}



impl AiChatFile{
    pub fn new()->Self{
        Self{
            history:vec![
                AiChatMessages::new()
            ],
        }
    }
    pub fn load(data: &str)->Result<AiChatFile,String>{
        AiChatFile::deserialize_ron(data).map_err(|e| format!("{:?}", e))
    }
    
    pub fn to_string(&self)->String{
        self.serialize_ron()
    }
    
    pub fn clamp_slot(&self, slot:&mut usize){
        *slot = self.history.len().saturating_sub(1).min(*slot);
    }
    
    pub fn remove_slot(&mut self,  _cx:&mut Cx, history_slot:&mut usize){
        self.clamp_slot(history_slot);
        self.history.remove(*history_slot);
        self.clamp_slot(history_slot);
        if self.history.len() == 0{
            self.history.push(AiChatMessages::new());
        }
    }
        // ok what happens. 
    pub fn fork_chat_at(&mut self, _cx:&mut Cx, history_slot:&mut usize, at:usize, data:String ) {
        // alriught so first we clamp the history slot
        self.clamp_slot(history_slot);
        if at + 1 != self.history[*history_slot].messages.len() { // fork it first
            let mut clone = self.history[*history_slot].clone();
            clone.messages.truncate(at + 1);
            *history_slot += 1;
            self.history.insert(*history_slot, clone);
        }
        if let AiChatMessage::User(s) = &mut self.history[*history_slot].messages[at]{
            s.message = data
        }
        else{
            error!("fork_chat_at: last message is not user")
        }
        // 
    }
    
    pub fn set_base_context(&mut self, history_slot:usize, at:usize, base_context:&str){ 
        if let AiChatMessage::User(s) = &mut self.history[history_slot].messages[at]{
            s.base_context = base_context.to_string()
        }
        else{panic!()}
    }
    
    pub fn set_model(&mut self, history_slot:usize, at:usize, model:&str){ 
        if let AiChatMessage::User(s) = &mut self.history[history_slot].messages[at]{
            s.model = model.to_string()
        }
        else{panic!()}
    }
    
    pub fn set_project(&mut self, history_slot:usize, at:usize, project:&str){ 
        if let AiChatMessage::User(s) = &mut self.history[history_slot].messages[at]{
            s.project = project.to_string()
        }
        else{panic!()}
    }
    
    pub fn set_auto_run(&mut self, history_slot:usize, at:usize, auto_run:bool){ 
        if let AiChatMessage::User(s) = &mut self.history[history_slot].messages[at]{
            s.auto_run = auto_run
        }
        else{panic!()}
    }
        
}

const AI_PROMPT_FILE:&'static str = "studio/resources/ai/ai_markup.txt";

impl AiChatManager{
    pub fn init(&mut self, fs:&mut FileSystem) {
        // lets load up all our context files
        for ctx in &self.contexts{
            for file in &ctx.files{
                if let Some(file_id) = fs.path_to_file_node_id(&file.path){
                    fs.request_open_file(LiveId(0), file_id);
                }
                else{
                    println!("Cant find {} in context {}",file.path,ctx.name);
                }
            }
        }
        for prj in &self.projects{
            for file in &prj.files{
                if let Some(file_id) = fs.path_to_file_node_id(&file.path){
                    fs.request_open_file(LiveId(0), file_id);
                }
                else{
                    println!("Cant find {} in context {}",file.path,prj.name);
                }
            }
        }
        if let Some(file_id) = fs.path_to_file_node_id(AI_PROMPT_FILE){
            fs.request_open_file(LiveId(0), file_id);
        }
    }
    
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event, fs:&mut FileSystem) {
        // lets handle the 
        
        // alright. lets see if we have any incoming Http things
        match event{
            Event::NetworkResponses(e)=>for e in e{
                // lets check our in flight queries
                if let Some((chat_id,OpenDocument::AiChat(doc))) = fs.open_documents.iter_mut().find(
                    |(_,v)| if let OpenDocument::AiChat(v) = v {if let Some(v) = &v.in_flight{v.request_id == e.request_id}else{false}} else{false}){
                        
                    let chat_id = *chat_id;
                    let in_flight = doc.in_flight.as_ref().unwrap();
                    match &e.response{
                        NetworkResponse::HttpRequestError(_err)=>{
                            println!("HTTP ERROR {:?}", _err);
                        }
                        NetworkResponse::HttpStreamResponse(res)=>{
                            let data = res.get_string_body().unwrap();
                                                        
                            let mut changed = false;
                            match in_flight.backend{
                                AiBackend::OpenAI{..}=>{
                                    for data in data.split("\n\n"){
                                        if let Some(data) = data.strip_prefix("data: "){
                                            if data != "[DONE]"{
                                                match OpenAiChatResponse::deserialize_json(data){
                                                    Ok(chat_response)=>{
                                                        if let Some(content) = &chat_response.choices[0].delta.as_ref().unwrap().content{
                                                            if let Some(msg) = doc.file.history.get_mut(in_flight.history_slot){
                                                                if let Some(AiChatMessage::Assistant(s)) = msg.messages.last_mut(){
                                                                    s.push_str(&content);
                                                                }
                                                                else{
                                                                    msg.messages.push(AiChatMessage::Assistant(content.clone()))
                                                                }
                                                            }
                                                            changed = true;
                                                        }
                                                    }
                                                    Err(e)=>{
                                                        println!("JSon parse error {:?} {}", e, data);
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                                AiBackend::Google{..}=>{
                                    println!("GOT {:?}", data);
                                    for data in data.split("\n\n"){
                                        if let Some(data) = data.strip_prefix("data: "){
                                            match GoogleAiResponse::deserialize_json(&data){
                                                Ok(response)=>{
                                                    for candidate in &response.candidates{
                                                        for part in &candidate.content.parts{
                                                            if let Some(msg) = doc.file.history.get_mut(in_flight.history_slot){
                                                                if let Some(AiChatMessage::Assistant(s)) = msg.messages.last_mut(){
                                                                    s.push_str(&part.text);
                                                                }
                                                                else{
                                                                    msg.messages.push(AiChatMessage::Assistant(part.text.clone()))
                                                                }
                                                            }
                                                            changed = true;
                                                        }
                                                    }
                                                }
                                                Err(e)=>{
                                                    println!("JSon parse error {:?} {}", e, data);
                                                }
                                            }
                                        }   
                                    }
                                }
                            }

                            if changed{
                                cx.action(AppAction::RedrawAiChat{chat_id});
                                cx.action(AppAction::SaveAiChat{chat_id});
                                //fs.request_save_file_for_file_node_id(chat_id, false);
                            }
                        }
                        NetworkResponse::HttpStreamComplete(_res)=>{
                            // done?..
                           //let chat_id = res.metadata_id;
                            // alright lets fetch the chat object
                            if let Some(OpenDocument::AiChat(doc)) = fs.open_documents.get_mut(&chat_id){
                                if let Some(in_flight) = doc.in_flight.take(){
                                    doc.in_flight = None;
                                    doc.file.history[in_flight.history_slot].follow_up();
                                    cx.action(AppAction::RedrawAiChat{chat_id});
                                    cx.action(AppAction::SaveAiChat{chat_id});
                                    
                                    if doc.auto_run{
                                        let item_id = doc.file.history[in_flight.history_slot].messages.len().saturating_sub(3);
                                        // lets check it auto_run = true
                                        cx.action(AppAction::RunAiChat{chat_id, history_slot:in_flight.history_slot, item_id});
                                        
                                    }
                                    // alright so we're done.. check if we have run-when-done
                                    doc.file.history[in_flight.history_slot].follow_up();
                                                                       
                                    //self.redraw_ai_chat_by_id(cx, chat_id, ui, fs);
                                    //fs.request_save_file_for_file_node_id(chat_id, false);
                                }
                            }
                        }
                        _=>{}
                    }
                }
            }
            _=>()
        }
    }
    
    pub fn run_ai_chat(&self, cx:&mut Cx, chat_id:LiveId, history_slot:usize, item_id:usize, fs:&mut FileSystem){
        if let Some(OpenDocument::AiChat(doc)) = fs.open_documents.get(&chat_id){
            let usr = doc.file.history[history_slot].messages.iter().nth(item_id);
            let ast = doc.file.history[history_slot].messages.iter().nth(item_id+1);
            if let Some(AiChatMessage::Assistant(ast)) = ast.cloned(){
                if let Some(AiChatMessage::User(usr)) = usr.cloned(){
                    
                    // lets check the project and the mode
                    println!("{:?}", usr);
                    
                    if let Some(project) = self.projects.iter().find(|v| v.name == usr.project){
                        if let Some(first) = project.files.get(0){
                            //let file_path =  "examples/simple/src/app.rs";
                            let file_id = fs.path_to_file_node_id(&first.path).unwrap();
                            //let old_data = fs.file_id_as_string(file_id).unwrap();
                            if let Some(new_data) = ast.strip_prefix("```rust"){
                                if let Some(new_data) = new_data.strip_suffix("```"){
                                    // alright depending
                                    if let Some(ctx) = self.contexts.iter().find(|v| v.name == usr.base_context){
                                        match ctx.apply{
                                            AiApply::PatchDSL=>{
                                                fs.replace_live_design(
                                                    cx,
                                                    file_id,
                                                    &new_data
                                                );
                                                fs.request_save_file_for_file_node_id(file_id, false);
                                                /*
                                                fs.process_possible_live_reload(
                                                    cx,
                                                    &first.path,
                                                    &old_data,
                                                    &new_data,
                                                    false
                                                );*/
                                            }
                                            AiApply::WholeFile=>{
                                                fs.replace_code_document(file_id, new_data);
                                                fs.request_save_file_for_file_node_id(file_id, false);
                                            }
                                            _=>()
                                        }
                                    }
                                }
                            }
                        }
                    }
                    
                }
            }
        }
    }
    
    pub fn cancel_chat_generation(&mut self, cx:&mut Cx, ui: &WidgetRef, chat_id:LiveId, fs:&mut FileSystem) {
        if let Some(OpenDocument::AiChat(doc)) = fs.open_documents.get_mut(&chat_id){
            if let Some(in_flight) = doc.in_flight.take(){
                cx.cancel_http_request(in_flight.request_id);
                if let Some(msg) = doc.file.history.get_mut(in_flight.history_slot){
                    msg.follow_up();
                    self.redraw_ai_chat_by_id(cx, chat_id, ui, fs);
                    cx.action(AppAction::SaveAiChat{chat_id});
                }
            }
        }
    }
    
    pub fn send_chat_to_backend(&mut self, cx: &mut Cx, chat_id:LiveId, history_slot:usize, fs:&mut FileSystem) {
        // build the request
        let (request,backend) = if let Some(OpenDocument::AiChat(doc)) = fs.open_documents.get(&chat_id){
            // alright lets fetch which backend we want
            let ai_model = if let Some(AiChatMessage::User(msg)) = doc.file.history[history_slot].messages.last(){
                if let Some(backend) = self.models.iter().find(|v| v.name == msg.model){
                    backend
                }
                else{
                    self.models.first().unwrap()
                }
            }
            else{
                self.models.first().unwrap()
            };
            match &ai_model.backend{
                AiBackend::OpenAI{url, model, key, reasoning_effort}=>{
                    
                    let mut request = HttpRequest::new(url.clone(), HttpMethod::POST);
                    request.set_is_streaming();
                    request.set_header("Authorization".to_string(), format!("Bearer {key}"));
                    request.set_header("Content-Type".to_string(), "application/json".to_string());
                    request.set_metadata_id(chat_id); 
                    let mut messages = Vec::new();
                    for msg in &doc.file.history[history_slot].messages{
                        match msg{
                            AiChatMessage::User(v)=>{
                                let doc = fs.file_path_as_string(AI_PROMPT_FILE).unwrap();
                                // parse it as html
                                let html = makepad_html::parse_html(&doc, &mut None, InternLiveId::No);
                                let html = html.new_walker();
                                
                                
                                // alright. we have to now collect our base context files
                                // ok lets find these files
                                let mut system_post = LiveId(0);
                                let mut general_post = LiveId(0);
                                                                
                                if let Some(ctx) = self.contexts.iter().find(|ctx| ctx.name == v.base_context){
                                    // alright lets fetch things
                                    system_post = ctx.system_post;
                                    general_post = ctx.general_post;
                                    
                                    if let Some(text) = html.find_tag_text(ctx.system_pre){
                                        messages.push(OpenAiChatMessage {content: Some(text.to_string()), role: Some("user".to_string()), refusal: Some(JsonValue::Null)});
                                    }
                                        
                                    for file in &ctx.files{
                                        if let Some(file_id) = fs.path_to_file_node_id(&file.path){
                                            if let Some(OpenDocument::Code(doc)) = fs.open_documents.get(&file_id){
                                                let mut content = String::new();
                                                let text = doc.as_text().to_string();
                                                content.push_str(&format!("\n Now follows a context file with description: ```{}``` given as context to help generating correct code. The filename is ```{}```\n",file.kind, file.path));
                                                content.push_str("```rust\n");
                                                content.push_str(&text);
                                                content.push_str("```\n");
                                                 messages.push(OpenAiChatMessage {content: Some(content), role: Some("user".to_string()), refusal: Some(JsonValue::Null)})
                                            }
                                        }
                                    }
                                }
                                messages.push(OpenAiChatMessage {content: Some(v.message.clone()), role: Some("user".to_string()), refusal: Some(JsonValue::Null)});
                                
                                if let Some(text) = html.find_tag_text(system_post){
                                    messages.push(OpenAiChatMessage {content: Some(text.to_string()), role: Some("user".to_string()), refusal: Some(JsonValue::Null)});
                                }
                                if let Some(text) = html.find_tag_text(general_post){
                                    messages.push(OpenAiChatMessage {content: Some(text.to_string()), role: Some("user".to_string()), refusal: Some(JsonValue::Null)});
                                }
                            }
                            AiChatMessage::Assistant(v)=>{
                                messages.push(OpenAiChatMessage {content: Some(v.clone()), role: Some("assistant".to_string()), refusal: Some(JsonValue::Null)})
                            }
                        }
                    }
                    request.set_json_body(OpenAiChatPrompt {
                        messages,
                        model: model.to_string(),
                        reasoning_effort:reasoning_effort.clone(),
                        max_tokens: 10000,
                        stream: true,
                    });
                    (request, ai_model.backend.clone())
                }
                AiBackend::Google{url, key}=>{
                    let mut request = HttpRequest::new(format!("{}{}", url.clone(),key), HttpMethod::POST);
                    request.set_is_streaming();
                    request.set_header("Content-Type".to_string(), "application/json".to_string());
                    request.set_metadata_id(chat_id); 
                    let mut contents = Vec::new();
                    for msg in &doc.file.history[history_slot].messages{
                        match msg{
                            AiChatMessage::User(v)=>{
                                let doc = fs.file_path_as_string(AI_PROMPT_FILE).unwrap();
                                // parse it as html
                                let html = makepad_html::parse_html(&doc, &mut None, InternLiveId::No);
                                let html = html.new_walker();
                                
                                // alright. we have to now collect our base context files
                                // ok lets find these files
                                let mut system_post = LiveId(0);
                                let mut general_post = LiveId(0);
                                                                                                
                                if let Some(ctx) = self.contexts.iter().find(|ctx| ctx.name == v.base_context){
                                    // alright lets fetch things
                                    system_post = ctx.system_post;
                                    general_post = ctx.general_post;
                                                                        
                                    if let Some(text) = html.find_tag_text(ctx.system_pre){
                                        contents.push(GoogleAiContent {
                                            parts: vec![GoogleAiPart{
                                                text:text.to_string()
                                            }],
                                            role: Some("user".to_string()), 
                                        });
                                    }
                                    
                                    let mut parts = Vec::new();
                                                                            
                                    for file in &ctx.files{
                                        if let Some(file_id) = fs.path_to_file_node_id(&file.path){
                                            if let Some(OpenDocument::Code(doc)) = fs.open_documents.get(&file_id){
                                                let mut content = String::new();
                                                let text = doc.as_text().to_string();
                                                content.push_str(&format!("The following is given as context to help generating correct code. The filename is ```{}```\n", file.path));
                                                content.push_str("```rust\n");
                                                content.push_str(&text);
                                                content.push_str("```\n");
                                                parts.push(GoogleAiPart{
                                                    text:content.to_string()
                                                })
                                                
                                            }
                                        }
                                    }
                                    if let Some(ctx) = self.projects.iter().find(|ctx| ctx.name == v.project){
                                        for file in &ctx.files{
                                            if let Some(file_id) = fs.path_to_file_node_id(&file.path){
                                                if let Some(OpenDocument::Code(doc)) = fs.open_documents.get(&file_id){
                                                    let mut content = String::new();
                                                    let text = doc.as_text().to_string();
                                                    content.push_str(&format!("The following is part the code to work on. The filename is ```{}```\n", file.path));
                                                    content.push_str("```rust\n");
                                                    content.push_str(&text);
                                                    content.push_str("```\n");
                                                    parts.push(GoogleAiPart{
                                                        text:content.to_string()
                                                    })
                                                }
                                            }
                                        }
                                    }
                                    contents.push(GoogleAiContent {
                                        parts,
                                        role: Some("user".to_string()), 
                                    });
                                }
                                contents.push(GoogleAiContent {
                                    parts: vec![GoogleAiPart{
                                        text:v.message.clone()
                                    }],
                                    role: Some("user".to_string()), 
                                });
                                                                
                                if let Some(text) = html.find_tag_text(system_post){
                                    contents.push(GoogleAiContent {
                                        parts: vec![GoogleAiPart{
                                            text:text.to_string()
                                        }],
                                        role: Some("user".to_string()), 
                                    });
                                }
                                if let Some(text) = html.find_tag_text(general_post){
                                    contents.push(GoogleAiContent {
                                        parts: vec![GoogleAiPart{
                                            text:text.to_string()
                                        }],
                                        role: Some("user".to_string()), 
                                    });
                                }
                            }
                            AiChatMessage::Assistant(v)=>{
                                contents.push(GoogleAiContent {
                                    parts: vec![GoogleAiPart{
                                        text:v.to_string()
                                    }],
                                    role: Some("model".to_string()), 
                                });
                            }
                        }
                    }
                    request.set_json_body(GoogleAiChatPrompt {
                        contents,
                    });
                    (request, ai_model.backend.clone())
                }
            }
        }
        else{
            panic!()
        };
        
        if let Some(OpenDocument::AiChat(doc)) = fs.open_documents.get_mut(&chat_id){
            let request_id = LiveId::unique();
            if let Some(in_flight) = doc.in_flight.take(){
                cx.cancel_http_request(in_flight.request_id);
            }
            doc.file.history[history_slot].last_time = Cx::time_now();
            doc.file.history[history_slot].messages.push(AiChatMessage::Assistant("".to_string()));
            doc.in_flight = Some(AiInFlight{
                history_slot,
                backend,
                request_id
            });
            cx.http_request(request_id, request);
        }
    }
    
    pub fn redraw_ai_chat_by_id(&mut self, cx: &mut Cx, chat_id: LiveId, ui: &WidgetRef, fs:&mut FileSystem) {
        // lets fetch all the sessions
        let dock = ui.dock(id!(dock));
        for (tab_id, file_node_id) in &fs.tab_id_to_file_node_id{
            if *file_node_id == chat_id{
                dock.item(*tab_id).redraw(cx);
            }
        }
    }
}