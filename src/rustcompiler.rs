use widget::*;
use editor::*;

use std::io::Read;
use std::process::{Command, Child, Stdio};
use std::sync::mpsc;
use std::collections::HashMap;

use serde_json::{Result};
use serde::*;

//#[derive(Clone)]
pub struct RustCompiler{
    pub view:View<ScrollBar>,
    pub bg:Quad,
    pub text:Text,
    pub item_bg:Quad,
    pub code_icon:CodeIcon,
    pub row_height:f32,
    pub _post_id:u64,
    pub _child:Option<Child>,
    pub _rx:Option<mpsc::Receiver<std::vec::Vec<u8>>>,
    pub _thread:Option<std::thread::JoinHandle<()>>,
    pub _data:Vec<String>,
    pub _rustc_spans:Vec<RustcSpan>,
    pub _rustc_messages:Vec<RustcCompilerMessage>,
    pub _rustc_done:bool,
    pub _rustc_artifacts:Vec<RustcCompilerArtifact>,
    pub _items:Vec<RustCompilerItem>
}

pub struct RustCompilerItem{
    hit_state:HitState,
    animator:Animator,
    marked:u64
}

#[derive(Clone)]
pub enum RustCompilerEvent{
    UpdateMessages,
    None,
}


impl Style for RustCompiler{
    fn style(cx:&mut Cx)->Self{
        Self{
            bg:Quad{
                ..Style::style(cx)
            },
            item_bg:Quad{
                ..Style::style(cx)
            },
            text:Text{
                ..Style::style(cx)
            },
            view:View{
                scroll_h:Some(ScrollBar{
                    ..Style::style(cx)
                }),
                scroll_v:Some(ScrollBar{
                    smoothing:Some(0.15),
                    ..Style::style(cx)
                }),
                ..Style::style(cx)
            },
            code_icon:CodeIcon{
                ..Style::style(cx)
            },
            row_height:20.0,
            _post_id:0,
            _child:None,
            _thread:None,
            _rx:None,
            _rustc_spans:Vec::new(),
            _rustc_messages:Vec::new(),
            _rustc_artifacts:Vec::new(),
            _rustc_done:false,
            _items:Vec::new(),
            _data:Vec::new()
        }
    }
}

impl RustCompiler{
    pub fn init(&mut self, cx:&mut Cx){
        self._post_id = cx.new_post_event_id();
        self.restart_rust_compiler();
    }

    pub fn get_default_anim(cx:&Cx, counter:usize, marked:bool)->Anim{
        Anim::new(Play::Chain{duration:0.01}, vec![
            Track::color("bg.color", Ease::Lin, vec![(1.0,
                if marked{cx.color("bg_marked")}  else if counter&1==0{cx.color("bg_selected")}else{cx.color("bg_odd")}
            )])
        ])
    }

    pub fn get_over_anim(cx:&Cx, counter:usize, marked:bool)->Anim{
        let over_color = if marked{cx.color("bg_marked_over")} else if counter&1==0{cx.color("bg_selected_over")}else{cx.color("bg_odd_over")};
        Anim::new(Play::Cut{duration:0.02}, vec![
            Track::color("bg.color", Ease::Lin, vec![
                (0., over_color),(1., over_color)
            ])
        ])
    }

    pub fn export_messages(&self, cx:&mut Cx, text_buffers:&mut TextBuffers){
        text_buffers.message_id += 1;
        let message_id = text_buffers.message_id;
        for span in &self._rustc_spans{
            if span.label.is_none(){
                continue;
            }
            // lets get the first span
            //let spans = &rcmsg.message.spans;
            //for i in (0..spans.len()).rev(){

                //let span = &spans[i];
            let path = format!("/{}",span.file_name);
            let text_buffer = text_buffers.from_path(cx, &path);
            text_buffer.message_mut_id = text_buffer.mutation_id;
            if text_buffer.message_id != message_id{
                text_buffer.message_id = message_id;
                text_buffer.message_cursors.truncate(0);
                text_buffer.message_bodies.truncate(0);
            }
            if span.byte_start == span.byte_end{
                text_buffer.message_cursors.push(TextCursor{
                    head:(span.byte_start-1) as usize,
                    tail:span.byte_end as usize,
                    max:0
                });
            }
            else{
                text_buffer.message_cursors.push(TextCursor{
                    head:span.byte_start as usize,
                    tail:span.byte_end as usize,
                    max:0
                });
            }
            //println!("PROCESING MESSAGES FOR {} {} {}", span.byte_start, span.byte_end+1, path);
            text_buffer.message_bodies.push(TextBufferMessage{
                body:span.label.clone().unwrap(),
                level:match span.level.as_ref().unwrap().as_ref(){
                    "warning"=>TextBufferMessageLevel::Warning,
                    "error"=>TextBufferMessageLevel::Error,
                    _=>TextBufferMessageLevel::Warning
                }
            });
            //}
        }
        // clear all files we missed
        for (_, text_buffer) in &mut text_buffers.storage{
            if text_buffer.message_id != message_id{
                text_buffer.message_id = message_id;
                text_buffer.message_cursors.truncate(0);
                text_buffer.message_bodies.truncate(0);
            }
        }
    }

    pub fn handle_rust_compiler(&mut self, cx:&mut Cx, event:&mut Event, text_buffers:&mut TextBuffers)->RustCompilerEvent{
        // do shit here
        if self.view.handle_scroll_bars(cx, event){
            // do zshit.
        }
        if let Event::Post(pe) = event{
            if self._post_id == pe.post_id{
                let mut datas = Vec::new();
                if let Some(rx) = &self._rx{
                    while let Ok(data) = rx.try_recv(){
                        datas.push(data);
                    }
                }
                if datas.len() > 0{
                    self.process_compiler_messages(cx, datas);
                    self.export_messages(cx, text_buffers);
                    return RustCompilerEvent::UpdateMessages;
                }

            }
            // lets export our errors
        }
        let mut unmark_nodes = false;
        let mut clicked_item = None;
        for (counter,item) in self._items.iter_mut().enumerate(){   
            match event.hits(cx, item.animator.area, &mut item.hit_state){
                Event::Animate(ae)=>{
                    item.animator.calc_write(cx, "bg.color", ae.time, item.animator.area);
                },
                Event::FingerDown(_fe)=>{
                    cx.set_down_mouse_cursor(MouseCursor::Hand);
                    // mark ourselves, unmark others
                    item.marked = cx.event_id;
                    unmark_nodes = true;
                    item.animator.play_anim(cx, Self::get_over_anim(cx, counter, item.marked != 0));
                    clicked_item = Some(counter);
                },
                Event::FingerUp(_fe)=>{
                },
                Event::FingerMove(_fe)=>{
                },
                Event::FingerHover(fe)=>{
                    cx.set_hover_mouse_cursor(MouseCursor::Hand);
                    match fe.hover_state{
                        HoverState::In=>{
                            item.animator.play_anim(cx, Self::get_over_anim(cx, counter, item.marked != 0));
                        },
                        HoverState::Out=>{
                            item.animator.play_anim(cx, Self::get_default_anim(cx, counter, item.marked != 0));
                        },
                        _=>()
                    }
                },
                _=>()
            }
        };
        if unmark_nodes{
            for (counter,item) in self._items.iter_mut().enumerate(){   
                if item.marked != cx.event_id{
                    item.marked = 0;
                    item.animator.play_anim(cx, Self::get_default_anim(cx, counter, false));
                }
            };
        }
        if let Some(clicked_item) = clicked_item{
            //return RustCompilerEvent::
        }
        RustCompilerEvent::None
    }

    pub fn draw_rust_compiler(&mut self, cx:&mut Cx){
        if let Err(_) = self.view.begin_view(cx, &Layout{..Default::default()}){
            return
        }

        //if !self._rustc_done{
        //    self.code_icon.draw_icon_walk(cx, CodeIconType::Warning);
        //    self.text.draw_text(cx, "Rust building...");
        //    cx.turtle_new_line();
        // }
        //else{
        //    self.text.draw_text(cx, "Rust Done!");
        //    cx.turtle_new_line();
        //}
        let mut counter = 0;
        for span in &self._rustc_spans{
            if span.label.is_none(){
                continue;
            }
            // reuse or overwrite a slot
             if counter >= self._items.len(){
                self._items.push(RustCompilerItem{
                    animator:Animator::new(Self::get_default_anim(cx, counter, false)),
                    hit_state:HitState{..Default::default()},
                    marked:0
                });
            };
            self.item_bg.color =  self._items[counter].animator.last_color("bg.color");

            let bg_inst = self.item_bg.begin_quad(cx,&Layout{
                width:Bounds::Fill,
                height:Bounds::Fix(self.row_height),
                padding:Padding{l:2.,t:3.,b:2.,r:0.},
                ..Default::default()
            });

            if let Some(level) = &span.level{
                if level == "error"{
                    self.code_icon.draw_icon_walk(cx, CodeIconType::Error);
                }
                else{
                    self.code_icon.draw_icon_walk(cx, CodeIconType::Warning);
                }
            }
            //if message.
            //self.text.draw_text(cx, &format!("{}", message.message.message));
            self.text.draw_text(cx, &format!("{}", span.label.as_ref().unwrap()));

            let bg_area = self.item_bg.end_quad(cx, &bg_inst);
            self._items[counter].animator.update_area_refs(cx, bg_area);

            cx.turtle_new_line();
            counter += 1;
        }

        // draw filler nodes
        
        let view_total = cx.get_turtle_bounds();   
        let rect_now =  cx.get_turtle_rect();
        let bg_even = cx.color("bg_selected");
        let bg_odd = cx.color("bg_odd");
        let mut y = view_total.y;
        while y < rect_now.h{
            self.item_bg.color = if counter&1 == 0{bg_even}else{bg_odd};
            self.item_bg.draw_quad_walk(cx,
                Bounds::Fill,
                Bounds::Fix( (rect_now.h - y).min(self.row_height) ),
                Margin::zero()
            );
            cx.turtle_new_line();
            y += self.row_height;
            counter += 1;
        } 

        self.view.end_view(cx);
    }

    pub fn restart_rust_compiler(&mut self){
        self._data.truncate(0);
        self._rustc_messages.truncate(0);
        self._rustc_spans.truncate(0);
        self._rustc_artifacts.truncate(0);
        self._rustc_done = false;
        self._items.truncate(0);
        self._data.push(String::new());

        if let Some(child) = &mut self._child{
            
            let _= child.kill();
        }

        let mut _child = Command::new("cargo")
            .args(&["check","--message-format=json"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .current_dir("./edit_repo")
            .spawn();
        
        if let Err(_) = _child{
            self._rustc_messages.push(RustcCompilerMessage{
                message:RustcMessage{
                    level:"error".to_string(),
                    message:"A Rust compiler error, only works native".to_string(),
                    ..Default::default()
                },
                ..Default::default()
            });
            self._rustc_messages.push(RustcCompilerMessage{
                message:RustcMessage{
                    level:"warning".to_string(),
                    message:"A Rust compiler warning, only works native".to_string(),
                    ..Default::default()
                },
                ..Default::default()
            });
            return;
        }
        let mut child = _child.unwrap();

        //let mut stderr =  child.stderr.take().unwrap();
        let mut stdout =  child.stdout.take().unwrap();
        let (tx, rx) = mpsc::channel();
        let post_id = self._post_id;
        let thread = std::thread::spawn(move ||{
            loop{
                let mut data = vec![0; 4096];
                let n_bytes_read = stdout.read(&mut data).expect("cannot read");
                data.truncate(n_bytes_read);
                let _ = tx.send(data);
                Cx::post_event(post_id, 0);
                if n_bytes_read == 0{
                    return 
                }
            }
        });
        self._rx = Some(rx);
        self._thread = Some(thread);
        self._child = Some(child);
    }

     pub fn process_compiler_messages(&mut self, cx:&mut Cx, datas:Vec<Vec<u8>>){
        for data in datas{
            if data.len() == 0{ // last event
                self._rustc_done = true;
                self.view.redraw_view_area(cx);
            }
            else {
                for ch in data{
                    if ch == '\n' as u8{
                        // parse it
                        let line = self._data.last_mut().unwrap();
                        // parse the line
                        if line.contains("\"reason\":\"compiler-artifact\""){
                            let parsed:Result<RustcCompilerArtifact> = serde_json::from_str(line); 
                            match parsed{
                                Err(err)=>println!("JSON PARSE ERROR {:?} {}", err, line),
                                Ok(parsed)=>{
                                    self._rustc_artifacts.push(parsed);
                                }
                            }
                            self.view.redraw_view_area(cx);
                        }
                        else if line.contains("\"reason\":\"compiler-message\""){
                            let parsed:Result<RustcCompilerMessage> = serde_json::from_str(line); 
                            match parsed{
                                Err(err)=>println!("JSON PARSE ERROR {:?} {}", err, line),
                                Ok(parsed)=>{
                                    let spans = &parsed.message.spans;
                                    if spans.len() > 0{
                                        for i in 0..spans.len(){
                                            let mut span = spans[i].clone();
                                            if !span.is_primary{
                                                continue
                                            }
                                            if span.label.is_none(){
                                                span.label = Some(parsed.message.message.clone());
                                            }
                                            span.level = Some(parsed.message.level.clone());
                                            self._rustc_spans.push(span);
                                        }
                                    }
                                    self._rustc_messages.push(parsed);
                                }
                            }
                            self.view.redraw_view_area(cx);
                        }
                        self._data.push(String::new());
                    }
                    else{
                        self._data.last_mut().unwrap().push(ch as char);
                    }
                }
            }
        }
    }
}

#[derive(Clone, Deserialize, Debug, Default)]
pub struct RustcTarget{
    kind:Vec<String>,
    crate_types:Vec<String>,
    name:String,
    src_path:String,
    edition:String
}

#[derive(Clone, Deserialize, Debug, Default)]
pub struct RustcText{
    text:String,
    highlight_start:u32,
    highlight_end:u32
}

#[derive(Clone, Deserialize, Debug, Default)]
pub struct RustcSpan{
    file_name:String,
    byte_start:u32,
    byte_end:u32,
    line_start:u32,
    line_end:u32,
    column_start:u32,
    column_end:u32,
    is_primary:bool,
    text:Vec<RustcText>,
    label:Option<String>,
    suggested_replacement:Option<String>,
    sugggested_applicability:Option<String>,
    expansion:Option<String>,
    level:Option<String>
}

#[derive(Clone, Deserialize, Debug, Default)]
pub struct RustcCode{
    code:String,
    explanation:Option<String>
}

#[derive(Clone, Deserialize, Debug, Default)]
pub struct RustcMessage{
    message:String,
    code:Option<RustcCode>,
    level:String,
    spans:Vec<RustcSpan>,
    children:Vec<RustcMessage>,
    rendered:Option<String>
}

#[derive(Clone, Deserialize, Debug, Default)]
pub struct RustcProfile{
    opt_level:String,
    debuginfo:u32,
    debug_assertions:bool,
    overflow_checks:bool,
    test:bool
}

#[derive(Clone, Deserialize, Debug, Default)]
pub struct RustcCompilerMessage{
    reason:String,
    package_id:String,
    target:RustcTarget,
    message:RustcMessage
}

#[derive(Clone, Deserialize, Debug, Default)]
pub struct RustcCompilerArtifact{
    reason:String,
    package_id:String,
    target:RustcTarget,
    profile:RustcProfile,
    features:Vec<String>,
    filenames:Vec<String>,
    executable:Option<String>,
    fresh:bool
}
