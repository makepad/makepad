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
                    name: "gemini 2.5 pro".to_string(),
                    backend: AiBackend::Google{
                        url:"https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-pro-preview-03-25:streamGenerateContent?alt=sse&key=".to_string(),
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
                    name: "Makepad Game".to_string(),
                    apply: AiApply::WholeFile,
                    system_pre: live_id!(GAME_PRE),
                    system_post: live_id!(GAME_POST),
                    general_post: live_id!(GENERAL_POST),
                    files: vec![
                        AiContextFile::new("Snake game example","makepad/examples/snake/src/app.rs"),
                    ]
                },
                BaseContext{
                    name: "Makepad DSL".to_string(),
                    apply: AiApply::PatchDSL,
                    system_pre: live_id!(UI_PRE),
                    system_post: live_id!(UI_POST),
                    general_post: live_id!(GENERAL_POST),
                    files: vec![
                        AiContextFile::new("News feed example","makepad/examples/news_feed/src/app.rs"),
                        AiContextFile::new("Todo example","makepad/examples/todo/src/app.rs"),
                        AiContextFile::new("Simple example","makepad/examples/simple/src/app.rs"),
                        AiContextFile::new("Snake game example","makepad/examples/snake/src/app.rs"),
                        AiContextFile::new("Slides viewer example","makepad/examples/slides/src/app.rs"),
                    ]
                },
                BaseContext{
                    name: "Makepad DSL Long".to_string(),
                    apply: AiApply::PatchDSL,
                    system_pre: live_id!(UI_PRE),
                    system_post: live_id!(UI_POST),
                    general_post: live_id!(GENERAL_POST),
                    files: vec![
                        AiContextFile::new("News feed example","makepad/examples/news_feed/src/app.rs"),
                        AiContextFile::new("Todo example","makepad/examples/todo/src/app.rs"),
                        AiContextFile::new("Simple example","makepad/examples/simple/src/app.rs"),
                        AiContextFile::new("Snake game example","makepad/examples/snake/src/app.rs"),
                        AiContextFile::new("Slides viewer example","makepad/examples/slides/src/app.rs"),
                        AiContextFile::new("UI Examples","makepad/examples/ui_zoo/src/app.rs"),
                        AiContextFile::new("UI Examples","makepad/examples/ui_zoo/src/demofiletree.rs"),
                        AiContextFile::new("UI Examples","makepad/examples/ui_zoo/src/layout_templates.rs"),
                        AiContextFile::new("UI Examples","makepad/examples/ui_zoo/src/lib.rs"),
                        AiContextFile::new("UI Examples","makepad/examples/ui_zoo/src/main.rs"),
                        AiContextFile::new("UI Examples","makepad/examples/ui_zoo/src/tab_button.rs"),
                        AiContextFile::new("UI Examples","makepad/examples/ui_zoo/src/tab_checkbox.rs"),
                        AiContextFile::new("UI Examples","makepad/examples/ui_zoo/src/tab_commandtextinput.rs"),
                        AiContextFile::new("UI Examples","makepad/examples/ui_zoo/src/tab_desktopbutton.rs"),
                        AiContextFile::new("UI Examples","makepad/examples/ui_zoo/src/tab_dropdown.rs"),
                        AiContextFile::new("UI Examples","makepad/examples/ui_zoo/src/tab_filetree.rs"),
                        AiContextFile::new("UI Examples","makepad/examples/ui_zoo/src/tab_foldbutton.rs"),
                        AiContextFile::new("UI Examples","makepad/examples/ui_zoo/src/tab_html.rs"),
                        AiContextFile::new("UI Examples","makepad/examples/ui_zoo/src/tab_icon.rs"),
                        AiContextFile::new("UI Examples","makepad/examples/ui_zoo/src/tab_iconset.rs"),
                        AiContextFile::new("UI Examples","makepad/examples/ui_zoo/src/tab_image.rs"),
                        AiContextFile::new("UI Examples","makepad/examples/ui_zoo/src/tab_imageblend.rs"),
                        AiContextFile::new("UI Examples","makepad/examples/ui_zoo/src/tab_label.rs"),
                        AiContextFile::new("UI Examples","makepad/examples/ui_zoo/src/tab_layout.rs"),
                        AiContextFile::new("UI Examples","makepad/examples/ui_zoo/src/tab_linklabel.rs"),
                        AiContextFile::new("UI Examples","makepad/examples/ui_zoo/src/tab_markdown.rs"),
                        AiContextFile::new("UI Examples","makepad/examples/ui_zoo/src/tab_pageflip.rs"),
                        AiContextFile::new("UI Examples","makepad/examples/ui_zoo/src/tab_portallist.rs"),
                        AiContextFile::new("UI Examples","makepad/examples/ui_zoo/src/tab_radiobutton.rs"),
                        AiContextFile::new("UI Examples","makepad/examples/ui_zoo/src/tab_rotary.rs"),
                        AiContextFile::new("UI Examples","makepad/examples/ui_zoo/src/tab_rotatedimage.rs"),
                        AiContextFile::new("UI Examples","makepad/examples/ui_zoo/src/tab_scrollbar.rs"),
                        AiContextFile::new("UI Examples","makepad/examples/ui_zoo/src/tab_slider.rs"),
                        AiContextFile::new("UI Examples","makepad/examples/ui_zoo/src/tab_slidesview.rs"),
                        AiContextFile::new("UI Examples","makepad/examples/ui_zoo/src/tab_textinput.rs"),
                        AiContextFile::new("UI Examples","makepad/examples/ui_zoo/src/tab_view.rs"),
                        AiContextFile::new("UI Examples","makepad/examples/ui_zoo/src/tab_widgetsoverview.rs"),
                        AiContextFile::new("UI Examples","makepad/examples/ui_zoo/src/uizoolayouts.rs"),
                        AiContextFile::new("Widget source","makepad/widgets/src/button.rs"),
                        AiContextFile::new("Widget source","makepad/widgets/src/check_box.rs"),
                        AiContextFile::new("Widget source","makepad/widgets/src/dock.rs"),
                        AiContextFile::new("Widget source","makepad/widgets/src/drop_down.rs"),
                        AiContextFile::new("Widget source","makepad/widgets/src/expandable_panel.rs"),
                        AiContextFile::new("Widget source","makepad/widgets/src/file_tree.rs"),
                        AiContextFile::new("Widget source","makepad/widgets/src/flat_list.rs"),
                        AiContextFile::new("Widget source","makepad/widgets/src/portal_list.rs"),
                        AiContextFile::new("Widget source","makepad/widgets/src/html.rs"),
                        AiContextFile::new("Widget source","makepad/widgets/src/image.rs"),
                        AiContextFile::new("Widget source","makepad/widgets/src/image_cache.rs"),
                        AiContextFile::new("Widget source","makepad/widgets/src/label.rs"),
                        AiContextFile::new("Widget source","makepad/widgets/src/link_label.rs"),
                        AiContextFile::new("Widget source","makepad/widgets/src/markdown.rs"),
                        AiContextFile::new("Widget source","makepad/widgets/src/portal_list.rs"),
                        AiContextFile::new("Widget source","makepad/widgets/src/radio_button.rs"),
                        AiContextFile::new("Widget source","makepad/widgets/src/scroll_bar.rs"),
                        AiContextFile::new("Widget source","makepad/widgets/src/scroll_bars.rs"),
                        AiContextFile::new("Widget source","makepad/widgets/src/slider.rs"),
                        AiContextFile::new("Widget source","makepad/widgets/src/slides_view.rs"),
                        AiContextFile::new("Widget source","makepad/widgets/src/tab.rs"),
                        AiContextFile::new("Widget source","makepad/widgets/src/tab_bar.rs"),
                        AiContextFile::new("Widget source","makepad/widgets/src/tab_close_button.rs"),
                        AiContextFile::new("Widget source","makepad/widgets/src/text_flow.rs"),
                        AiContextFile::new("Widget source","makepad/widgets/src/text_input.rs"),
                        AiContextFile::new("Widget source","makepad/widgets/src/theme_desktop_dark.rs"),
                        AiContextFile::new("Widget source","makepad/widgets/src/view.rs"),
                        AiContextFile::new("Widget source","makepad/widgets/src/view_ui.rs"),
                        AiContextFile::new("Widget source","makepad/widgets/src/widget.rs"),
                        AiContextFile::new("Widget source","makepad/widgets/src/widget_match_event.rs"),
                        AiContextFile::new("Widget source","makepad/widgets/src/window.rs"),
                    ]
                },
                BaseContext{
                    name: "Makepad Rust".to_string(),
                    apply: AiApply::WholeFile,
                    system_pre: live_id!(ALL_PRE),
                    system_post: live_id!(ALL_POST),
                    general_post: live_id!(GENERAL_POST),
                    files: vec![
                    ]
                },
                BaseContext{
                    name: "Chat".to_string(),
                    apply: AiApply::None,
                    system_pre: live_id!(CHAT_PRE),
                    system_post: live_id!(CHAT_POST),
                    general_post: live_id!(CHAT_GENERAL),
                    files: vec![]
                },
                BaseContext{
                    name: "Makepad Internal".to_string(),
                    apply: AiApply::WholeFile,
                    system_pre: live_id!(INTERNAL_PRE),
                    system_post: live_id!(INTERNAL_POST),
                    general_post: live_id!(INTERNAL_GENERAL),
                    files: vec![                   
                        AiContextFile::new("","makepad/platform/live_compiler/src/live_document.rs"),
                        AiContextFile::new("","makepad/platform/live_compiler/src/live_error.rs"),
                        AiContextFile::new("","makepad/platform/live_compiler/src/live_eval.rs"),
                        AiContextFile::new("","makepad/platform/live_compiler/src/live_expander.rs"),
                        AiContextFile::new("","makepad/platform/live_compiler/src/live_node.rs"),
                        AiContextFile::new("","makepad/platform/live_compiler/src/live_node_vec.rs"),
                        AiContextFile::new("","makepad/platform/live_compiler/src/live_parser.rs"),
                        AiContextFile::new("","makepad/platform/live_compiler/src/live_ptr.rs"),
                        AiContextFile::new("","makepad/platform/live_compiler/src/live_registry.rs"),
                        AiContextFile::new("","makepad/platform/live_compiler/src/live_token.rs"),
                        AiContextFile::new("","makepad/platform/live_compiler/src/span.rs"),
                        AiContextFile::new("","makepad/platform/live_compiler/src/util.rs"),
                        
                        AiContextFile::new("","makepad/platform/live_tokenizer/src/char_ext.rs"),
                        AiContextFile::new("","makepad/platform/live_tokenizer/src/colorhex.rs"),
                        AiContextFile::new("","makepad/platform/live_tokenizer/src/full_token.rs"),
                        AiContextFile::new("","makepad/platform/live_tokenizer/src/live_error_origin.rs"),
                        AiContextFile::new("","makepad/platform/live_tokenizer/src/tokenizer.rs"),
                        AiContextFile::new("","makepad/platform/live_tokenizer/src/vec4_ext.rs"),
                        
                        AiContextFile::new("","makepad/platform/shader_compiler/src/analyse.rs"),
                        AiContextFile::new("","makepad/platform/shader_compiler/src/builtin.rs"),
                        AiContextFile::new("","makepad/platform/shader_compiler/src/const_eval.rs"),
                        AiContextFile::new("","makepad/platform/shader_compiler/src/const_gather.rs"),
                        AiContextFile::new("","makepad/platform/shader_compiler/src/dep_analyse.rs"),
                        AiContextFile::new("","makepad/platform/shader_compiler/src/generate.rs"),
                        AiContextFile::new("","makepad/platform/shader_compiler/src/generate_glsl.rs"),
                        AiContextFile::new("","makepad/platform/shader_compiler/src/generate_hlsl.rs"),
                        AiContextFile::new("","makepad/platform/shader_compiler/src/generate_metal.rs"),
                        AiContextFile::new("","makepad/platform/shader_compiler/src/lhs_check.rs"),
                        AiContextFile::new("","makepad/platform/shader_compiler/src/shader_ast.rs"),
                        AiContextFile::new("","makepad/platform/shader_compiler/src/shader_parser.rs"),
                        AiContextFile::new("","makepad/platform/shader_compiler/src/shader_registry.rs"),
                        AiContextFile::new("","makepad/platform/shader_compiler/src/swizzle.rs"),
                        AiContextFile::new("","makepad/platform/shader_compiler/src/ty_check.rs"),
                        AiContextFile::new("","makepad/platform/shader_compiler/src/util.rs"),
                        
                        AiContextFile::new("","makepad/platform/src/event/designer.rs"),
                        AiContextFile::new("","makepad/platform/src/event/drag_drop.rs"),
                        AiContextFile::new("","makepad/platform/src/event/event.rs"),
                        AiContextFile::new("","makepad/platform/src/event/finger.rs"),
                        AiContextFile::new("","makepad/platform/src/event/keyboard.rs"),
                        AiContextFile::new("","makepad/platform/src/event/network.rs"),
                        AiContextFile::new("","makepad/platform/src/event/video_playback.rs"),
                        AiContextFile::new("","makepad/platform/src/event/window.rs"),
                                                                        
                        AiContextFile::new("","makepad/platform/src/action.rs"),
                        AiContextFile::new("","makepad/platform/src/animator.rs"),
                        AiContextFile::new("","makepad/platform/src/app_main.rs"),
                        AiContextFile::new("","makepad/platform/src/area.rs"),
                        AiContextFile::new("","makepad/platform/src/cx.rs"),
                        AiContextFile::new("","makepad/platform/src/cx_api.rs"),
                        AiContextFile::new("","makepad/platform/src/debug.rs"),
                        
                        AiContextFile::new("","makepad/platform/src/draw_list.rs"),
                        AiContextFile::new("","makepad/platform/src/draw_shader.rs"),
                        AiContextFile::new("","makepad/platform/src/draw_vars.rs"),
                        
                        AiContextFile::new("","makepad/platform/src/file_dialogs.rs"),
                        AiContextFile::new("","makepad/platform/src/geometry.rs"),
                        AiContextFile::new("","makepad/platform/src/gpu_info.rs"),
                        AiContextFile::new("","makepad/platform/src/id_pool.rs"),
                        AiContextFile::new("","makepad/platform/src/live_atomic.rs"),
                        AiContextFile::new("","makepad/platform/src/live_cx.rs"),
                        AiContextFile::new("","makepad/platform/src/live_prims.rs"),
                        AiContextFile::new("","makepad/platform/src/live_traits.rs"),
                        AiContextFile::new("","makepad/platform/src/log.rs"),
                        AiContextFile::new("","makepad/platform/src/pass.rs"),
                        AiContextFile::new("","makepad/platform/src/scope.rs"),
                        AiContextFile::new("","makepad/platform/src/studio.rs"),
                        AiContextFile::new("","makepad/platform/src/texture.rs"),
                        AiContextFile::new("","makepad/platform/src/thread.rs"),
                        AiContextFile::new("","makepad/platform/src/ui_runner.rs"),
                        AiContextFile::new("","makepad/platform/src/video.rs"),
                        AiContextFile::new("","makepad/platform/src/web_socket.rs"),
                        AiContextFile::new("","makepad/platform/src/window.rs"),
                        
                        AiContextFile::new("","makepad/draw/src/cx_2d.rs"),
                        AiContextFile::new("","makepad/draw/src/draw_list_2d.rs"),
                        
                        AiContextFile::new("","makepad/draw/src/match_event.rs"),
                        AiContextFile::new("","makepad/draw/src/nav.rs"),
                        AiContextFile::new("","makepad/draw/src/overlay.rs"),
                        AiContextFile::new("","makepad/draw/src/shader/draw_color.rs"),
                        AiContextFile::new("","makepad/draw/src/shader/draw_quad.rs"),
                        AiContextFile::new("","makepad/draw/src/shader/draw_text.rs"),
                        AiContextFile::new("","makepad/draw/src/turtle.rs"),
                        AiContextFile::new("","makepad/widgets/src/button.rs"),
                        AiContextFile::new("","makepad/widgets/src/check_box.rs"),
                        AiContextFile::new("","makepad/widgets/src/dock.rs"),
                        AiContextFile::new("","makepad/widgets/src/drop_down.rs"),
                        AiContextFile::new("","makepad/widgets/src/expandable_panel.rs"),
                        AiContextFile::new("","makepad/widgets/src/file_tree.rs"),
                        AiContextFile::new("","makepad/widgets/src/flat_list.rs"),
                        AiContextFile::new("","makepad/widgets/src/portal_list.rs"),
                        AiContextFile::new("","makepad/widgets/src/html.rs"),
                        AiContextFile::new("","makepad/widgets/src/image.rs"),
                        AiContextFile::new("","makepad/widgets/src/image_cache.rs"),
                        AiContextFile::new("","makepad/widgets/src/label.rs"),
                        AiContextFile::new("","makepad/widgets/src/link_label.rs"),
                        AiContextFile::new("","makepad/widgets/src/markdown.rs"),
                        AiContextFile::new("","makepad/widgets/src/portal_list.rs"),
                        AiContextFile::new("","makepad/widgets/src/radio_button.rs"),
                        AiContextFile::new("","makepad/widgets/src/scroll_bar.rs"),
                        AiContextFile::new("","makepad/widgets/src/scroll_bars.rs"),
                        AiContextFile::new("","makepad/widgets/src/slider.rs"),
                        AiContextFile::new("","makepad/widgets/src/slides_view.rs"),
                        AiContextFile::new("","makepad/widgets/src/tab.rs"),
                        AiContextFile::new("","makepad/widgets/src/tab_bar.rs"),
                        AiContextFile::new("","makepad/widgets/src/tab_close_button.rs"),
                        AiContextFile::new("","makepad/widgets/src/text_flow.rs"),
                        AiContextFile::new("","makepad/widgets/src/text_input.rs"),
                        AiContextFile::new("","makepad/widgets/src/theme_desktop_dark.rs"),
                        AiContextFile::new("","makepad/widgets/src/view.rs"),
                        AiContextFile::new("","makepad/widgets/src/view_ui.rs"),
                        AiContextFile::new("","makepad/widgets/src/widget.rs"),
                        AiContextFile::new("","makepad/widgets/src/widget_match_event.rs"),
                        AiContextFile::new("","makepad/widgets/src/window.rs"),
                        AiContextFile::new("","makepad/widgets/src/designer.rs"),
                        AiContextFile::new("","makepad/widgets/src/designer_data.rs"),
                        AiContextFile::new("","makepad/widgets/src/designer_dummy.rs"),
                        AiContextFile::new("","makepad/widgets/src/designer_outline.rs"),
                        AiContextFile::new("","makepad/widgets/src/designer_outline_tree.rs"),
                        AiContextFile::new("","makepad/widgets/src/designer_theme.rs"),
                        AiContextFile::new("","makepad/widgets/src/designer_toolbox.rs"),
                        AiContextFile::new("","makepad/widgets/src/designer_view.rs"),
                    ],
                },
            ],
            projects: vec![
                AiProject{
                    name:"None".to_string(),
                    files:vec![]
                },
                AiProject{
                    name:"makepad-experiment-ai-snake".to_string(),
                    files:vec![
                        AiContextFile::new("Main app to rewrite","ai_snake/src/app.rs")
                    ]
                },
                AiProject{
                    name:"makepad-experiment-ai-mr".to_string(),
                    files:vec![
                        AiContextFile::new("Main app to rewrite","ai_mr/src/app.rs")
                    ]
                },
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
    pub message:String
}

#[derive(Debug, SerRon, DeRon, Clone)]
pub enum AiChatMessage{
    User(AiUserMessage),
    Assistant(String)
}


#[derive(Debug, SerRon, DeRon, Clone,)]
pub struct AiChatMessages{
    pub context: Vec<AiContext>,
    pub auto_run: bool,
    pub model: String,
    pub project: String,
    pub base_context: String,
    pub last_time: f64,
    pub messages: Vec<AiChatMessage>
}

impl AiChatMessages{
    fn new()->Self{
        AiChatMessages{
            last_time: 0.0,
            auto_run: true,
            model: "".to_string(),
            project: "".to_string(),
            base_context: "".to_string(),
            context: vec![],
            messages: vec![AiChatMessage::User(AiUserMessage::default())],
        }
    }
    
    fn follow_up(&mut self){
       self.messages.push(AiChatMessage::User(AiUserMessage{
            message:"".to_string()
        }));
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
    
    pub fn set_base_context(&mut self, history_slot:usize, base_context:&str){ 
        self.history[history_slot].base_context = base_context.to_string()
    }
    
    pub fn set_model(&mut self, history_slot:usize, model:&str){ 
        self.history[history_slot].model = model.to_string()
    }
    
    pub fn set_project(&mut self, history_slot:usize, project:&str){ 
        self.history[history_slot].project = project.to_string()
    }
    
    pub fn set_auto_run(&mut self, history_slot:usize,  auto_run:bool){ 
        self.history[history_slot].auto_run = auto_run
    }
        
}

const AI_PROMPT_FILE:&'static str = "makepad/studio/resources/ai/ai_markup.txt";

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
                                    for data in data.split("\r\n\r\n"){
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
                                    //doc.file.history[in_flight.history_slot].follow_up();
                                                                       
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
            let messages = &doc.file.history[history_slot];
            let ast = messages.messages.iter().nth(item_id);
            let usr = messages.messages.iter().nth(item_id.saturating_sub(1));
                        
            if let Some(AiChatMessage::Assistant(ast)) = ast.cloned(){
                if let Some(project) = self.projects.iter().find(|v| v.name == messages.project){
                    if let Some(first) = project.files.get(0){
                        //let file_path =  "makepad/examples/simple/src/app.rs";
                        let file_id = fs.path_to_file_node_id(&first.path).unwrap();
                        //let old_data = fs.file_id_as_string(file_id).unwrap();
                        if let Some(new_data) = ast.strip_prefix("```rust"){
                            if let Some(new_data) = new_data.strip_suffix("```"){
                                // alright depending
                                // go set the snapshot textbox
                                if let Some(AiChatMessage::User(usr)) = usr.cloned(){
                                    cx.action(AppAction::SetSnapshotMessage{message:usr.message.clone()});
                                }
                                if let Some(ctx) = self.contexts.iter().find(|v| v.name == messages.base_context){
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
            let ai_model = if let Some(backend) = self.models.iter().find(|v| v.name == doc.file.history[history_slot].model){
                backend
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
                    let mut out_messages = Vec::new();
                    
                    let messages = &doc.file.history[history_slot];
                    
                    let ai_html = fs.file_path_as_string(AI_PROMPT_FILE).unwrap();
                    let html = makepad_html::parse_html(&ai_html, &mut None, InternLiveId::No);
                    let html = html.new_walker();
                    
                    // alright lets plug in the context 'head'
                    
                    if let Some(ctx) = self.contexts.iter().find(|ctx| ctx.name == messages.base_context){
                        // alright lets fetch things
                                                            
                        if let Some(text) = html.find_tag_text(ctx.system_pre){
                            out_messages.push(OpenAiChatMessage {content: Some(text.to_string()), role: Some("user".to_string()), refusal: Some(JsonValue::Null)});
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
                                    out_messages.push(OpenAiChatMessage {content: Some(content), role: Some("user".to_string()), refusal: Some(JsonValue::Null)})
                                }
                            }
                        }
                        
                        if let Some(text) = html.find_tag_text(ctx.system_post){
                            out_messages.push(OpenAiChatMessage {content: Some(text.to_string()), role: Some("user".to_string()), refusal: Some(JsonValue::Null)});
                        }                      
                        
                        for msg in &messages.messages{
                            match msg{
                                AiChatMessage::User(v)=>{
                                    out_messages.push(OpenAiChatMessage {content: Some(v.message.clone()), role: Some("user".to_string()), refusal: Some(JsonValue::Null)});
                                }
                                AiChatMessage::Assistant(v)=>{
                                    out_messages.push(OpenAiChatMessage {content: Some(v.clone()), role: Some("assistant".to_string()), refusal: Some(JsonValue::Null)})
                                }
                            }
                        }
                        
                        if let Some(text) = html.find_tag_text(ctx.general_post){
                            out_messages.push(OpenAiChatMessage {content: Some(text.to_string()), role: Some("user".to_string()), refusal: Some(JsonValue::Null)});
                        }
                    }
                    
                    request.set_json_body(OpenAiChatPrompt {
                        messages: out_messages,
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
                    
                    let messages = &doc.file.history[history_slot];
                                        
                    let ai_html = fs.file_path_as_string(AI_PROMPT_FILE).unwrap();
                    // parse it as html
                    let html = makepad_html::parse_html(&ai_html, &mut None, InternLiveId::No);
                    let html = html.new_walker();
                    
                    if let Some(ctx) = self.contexts.iter().find(|ctx| ctx.name == messages.base_context){
                            
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
                        
                        contents.push(GoogleAiContent {
                            parts,
                            role: Some("user".to_string()), 
                        });
                                                                                                
                        if let Some(text) = html.find_tag_text(ctx.system_post){
                            contents.push(GoogleAiContent {
                                parts: vec![GoogleAiPart{
                                    text:text.to_string()
                                }],
                                role: Some("user".to_string()), 
                            });
                        }
                        
                        for msg in &messages.messages{
                            match msg{
                                AiChatMessage::User(v)=>{
                                    contents.push(GoogleAiContent {
                                        parts: vec![GoogleAiPart{
                                            text:v.message.clone()
                                        }],
                                        role: Some("user".to_string()), 
                                    });
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
                        
                        if let Some(text) = html.find_tag_text(ctx.general_post){
                            contents.push(GoogleAiContent {
                                parts: vec![GoogleAiPart{
                                    text:text.to_string()
                                }],
                                role: Some("user".to_string()), 
                            });
                        }
                    }
                    let contents = GoogleAiChatPrompt {
                        contents,
                    };
                    request.set_json_body(contents);
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