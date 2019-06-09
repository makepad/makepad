use render::*;
use widget::*;
use editor::*;

use std::io::Read;
use std::sync::mpsc;
use std::process:: {Child};

use serde_json:: {Result};
use serde::*;

//#[derive(Clone)]
pub struct RustCompiler {
    pub view: View<ScrollBar>,
    pub bg: Quad,
    pub text: Text,
    pub item_bg: Quad,
    pub code_icon: CodeIcon,
    pub row_height: f32,
    pub path_color: Color,
    pub message_color: Color,
    pub _check_signal: Signal,
    
    pub _check_child: Option<Child>,
    pub _build_child: Option<Child>,
    pub _run_child: Option<Child>,
    pub _run_when_done: bool,
    pub _program_running: bool,
    pub _messages_updated: bool,
    
    pub _rustc_build_stages: BuildStage,
    pub _rx: Option<mpsc::Receiver<std::vec::Vec<u8>>>,
    
    pub _thread: Option<std::thread::JoinHandle<()>>,
    
    pub _data: Vec<String>,
    pub _visible_window: (usize, usize),
    pub _draw_messages: Vec<RustDrawMessage>,
    // pub _rustc_spans:Vec<RustcSpan>,
    pub _rustc_messages: Vec<RustcCompilerMessage>,
    pub _rustc_artifacts: Vec<RustcCompilerArtifact>,
    pub _rustc_done: bool,
}

const SIGNAL_RUST_CHECKER: u64 = 1;
const SIGNAL_BUILD_COMPLETE: u64 = 2;
const SIGNAL_RUN_OUTPUT: u64 = 3;

#[derive(PartialEq, Debug)]
pub enum BuildStage {
    NotRunning,
    Building,
    Complete
}

#[derive(Clone)]
pub struct RustDrawMessage {
    hit_state: HitState,
    animator: Animator,
    path: String,
    body: String,
    more_lines: Vec<String>,
    row: usize,
    col: usize,
    head: usize,
    tail: usize,
    level: TextBufferMessageLevel,
    is_selected: bool
}

#[derive(Clone)]
pub enum RustCompilerEvent {
    SelectMessage {path: String},
    None,
}

impl Style for RustCompiler {
    fn style(cx: &mut Cx) -> Self {
        Self {
            bg: Quad {
                ..Style::style(cx)
            },
            item_bg: Quad {
                ..Style::style(cx)
            },
            text: Text {
                wrapping: Wrapping::Word,
                ..Style::style(cx)
            },
            view: View {
                scroll_h: Some(ScrollBar {
                    ..Style::style(cx)
                }),
                scroll_v: Some(ScrollBar {
                    smoothing: Some(0.15),
                    ..Style::style(cx)
                }),
                ..Style::style(cx)
            },
            code_icon: CodeIcon {
                ..Style::style(cx)
            },
            path_color: color("#999"),
            message_color: color("#bbb"),
            row_height: 20.0,
            _check_signal: Signal::empty(),
            _check_child: None,
            _build_child: None,
            _run_child: None,
            _run_when_done: false,
            _program_running: false,
            _rustc_build_stages: BuildStage::NotRunning,
            _thread: None,
            _rx: None,
            //_rustc_spans:Vec::new(),
            _draw_messages: Vec::new(),
            _messages_updated: true,
            _visible_window: (0, 0),
            _rustc_messages: Vec::new(),
            _rustc_artifacts: Vec::new(),
            _rustc_done: false,
            //_items:Vec::new(),
            _data: Vec::new()
        }
    }
}

impl RustCompiler {
    pub fn init(&mut self, cx: &mut Cx, text_buffers: &mut TextBuffers) {
        self._check_signal = cx.new_signal();
        self.restart_rust_checker(cx, text_buffers);
    }
    
    pub fn get_default_anim(cx: &Cx, counter: usize, marked: bool) -> Anim {
        Anim::new(Play::Chain {duration: 0.01}, vec![
            Track::color("bg.color", Ease::Lin, vec![(1.0, if marked {cx.color("bg_marked")} else if counter & 1 == 0 {cx.color("bg_selected")}else {cx.color("bg_odd")})])
        ])
    }
    
    pub fn get_over_anim(cx: &Cx, counter: usize, marked: bool) -> Anim {
        let over_color = if marked {cx.color("bg_marked_over")} else if counter & 1 == 0 {cx.color("bg_selected_over")}else {cx.color("bg_odd_over")};
        Anim::new(Play::Cut {duration: 0.02}, vec![
            Track::color("bg.color", Ease::Lin, vec![
                (0., over_color),
                (1., over_color)
            ])
        ])
    }
    
    fn clear_textbuffer_messages(&self, cx: &mut Cx, text_buffers: &mut TextBuffers) {
        // clear all files we missed
        for (_, text_buffer) in &mut text_buffers.storage {
            if text_buffer.messages.gc_id != cx.event_id {
                text_buffer.messages.cursors.truncate(0);
                text_buffer.messages.bodies.truncate(0);
                cx.send_signal_before_draw(text_buffer.signal, SIGNAL_TEXTBUFFER_MESSAGE_UPDATE);
            }
            else {
                cx.send_signal_before_draw(text_buffer.signal, SIGNAL_TEXTBUFFER_MESSAGE_UPDATE);
            }
        }
    }
    
    pub fn export_messages(&self, cx: &mut Cx, text_buffers: &mut TextBuffers) {
        
        for dm in &self._draw_messages {
            
            let text_buffer = text_buffers.from_path(cx, &dm.path);
            
            let messages = &mut text_buffer.messages;
            messages.mutation_id = text_buffer.mutation_id;
            if messages.gc_id != cx.event_id {
                messages.gc_id = cx.event_id;
                messages.cursors.truncate(0);
                messages.bodies.truncate(0);
            }
            
            if dm.level == TextBufferMessageLevel::Log {
                break
            }
            if dm.head == dm.tail {
                messages.cursors.push(TextCursor {
                    head: dm.head as usize,
                    tail: dm.tail as usize,
                    max: 0
                })
            }
            else {
                messages.cursors.push(TextCursor {
                    head: dm.head,
                    tail: dm.tail,
                    max: 0
                })
            }
            
            //println!("PROCESING MESSAGES FOR {} {} {}", span.byte_start, span.byte_end+1, path);
            text_buffer.messages.bodies.push(TextBufferMessage {
                body: dm.body.clone(),
                level: dm.level.clone()
            });
            //}
        }
        self.clear_textbuffer_messages(cx, text_buffers);
        
    }
    
    pub fn handle_rust_compiler(&mut self, cx: &mut Cx, event: &mut Event, text_buffers: &mut TextBuffers) -> RustCompilerEvent {
        // do shit here
        if self.view.handle_scroll_bars(cx, event) {
            // do zshit.
        }
        
        let mut dm_to_select = None;
        
        match event {
            Event::KeyDown(ke) => match ke.key_code {
                KeyCode::F9 => {
                    if self._rustc_build_stages == BuildStage::Complete {
                        self.run_program();
                        println!("RUNNING PROGRAM!");
                    }
                    else {
                        println!("BUILDSTAGE {:?}", self._rustc_build_stages);
                        self._run_when_done = true;
                    }
                    self.view.redraw_view_area(cx);
                },
                KeyCode::F8 => { // next error
                    if self._draw_messages.len() > 0 {
                        if ke.modifiers.shift {
                            let mut selected_index = None;
                            for (counter, item) in self._draw_messages.iter_mut().enumerate() {
                                if item.is_selected {
                                    selected_index = Some(counter);
                                }
                            }
                            if let Some(selected_index) = selected_index {
                                if selected_index > 0 {
                                    dm_to_select = Some(selected_index - 1);
                                }
                                else {
                                    dm_to_select = Some(self._draw_messages.len() - 1);
                                }
                            }
                            else {
                                dm_to_select = Some(self._draw_messages.len() - 1);
                            }
                        }
                        else {
                            let mut selected_index = None;
                            for (counter, dm) in self._draw_messages.iter_mut().enumerate() {
                                if dm.is_selected {
                                    selected_index = Some(counter);
                                }
                            }
                            if let Some(selected_index) = selected_index {
                                if selected_index + 1 < self._draw_messages.len() {
                                    dm_to_select = Some(selected_index + 1);
                                }
                                else {
                                    dm_to_select = Some(0);
                                }
                            }
                            else {
                                dm_to_select = Some(0);
                            }
                        }
                    }
                },
                _ => ()
            },
            Event::Signal(se) => {
                if self._check_signal.is_signal(se) {
                    match se.value {
                        SIGNAL_RUST_CHECKER | SIGNAL_RUN_OUTPUT => {
                            let mut datas = Vec::new();
                            if let Some(rx) = &self._rx {
                                while let Ok(data) = rx.try_recv() {
                                    datas.push(data);
                                }
                            }
                            if datas.len() > 0 {
                                if se.value == SIGNAL_RUST_CHECKER {
                                    self.process_compiler_messages(cx, datas);
                                    self.export_messages(cx, text_buffers);
                                }
                                else {
                                    self.process_run_messages(cx, datas);
                                }
                            }
                        },
                        SIGNAL_BUILD_COMPLETE => {
                            self._rustc_build_stages = BuildStage::Complete;
                            if self._run_when_done {
                                self.run_program();
                            }
                            self.view.redraw_view_area(cx);
                        },
                        _ => ()
                    }
                }
            },
            _ => ()
        }
        
        //let mut unmark_nodes = false;
        for (counter, dm) in self._draw_messages.iter_mut().enumerate() {
            match dm.hit_state.hits(cx, dm.animator.area, event) {
                Event::Animate(ae) => {
                    dm.animator.calc_write(cx, "bg.color", ae.time, dm.animator.area);
                },
                Event::FingerDown(_fe) => {
                    cx.set_down_mouse_cursor(MouseCursor::Hand);
                    // mark ourselves, unmark others
                    dm_to_select = Some(counter);
                },
                Event::FingerUp(_fe) => {
                },
                Event::FingerMove(_fe) => {
                },
                Event::FingerHover(fe) => {
                    cx.set_hover_mouse_cursor(MouseCursor::Hand);
                    match fe.hover_state {
                        HoverState::In => {
                            dm.animator.play_anim(cx, Self::get_over_anim(cx, counter, dm.is_selected));
                        },
                        HoverState::Out => {
                            dm.animator.play_anim(cx, Self::get_default_anim(cx, counter, dm.is_selected));
                        },
                        _ => ()
                    }
                },
                _ => ()
            }
        };
        
        if let Some(dm_to_select) = dm_to_select {
            
            for (counter, dm) in self._draw_messages.iter_mut().enumerate() {
                if counter != dm_to_select {
                    dm.is_selected = false;
                    dm.animator.play_anim(cx, Self::get_default_anim(cx, counter, false));
                }
            };
            
            let dm = &mut self._draw_messages[dm_to_select];
            dm.is_selected = true;
            dm.animator.play_anim(cx, Self::get_over_anim(cx, dm_to_select, true));
            
            // alright we clicked an item. now what. well
            if dm.path != "" {
                let text_buffer = text_buffers.from_path(cx, &dm.path);
                text_buffer.messages.jump_to_offset = if dm.level == TextBufferMessageLevel::Log {
                    text_buffer.text_pos_to_offset(TextPos {row: dm.row - 1, col: dm.col - 1})
                }
                else {
                    dm.head
                };
                
                cx.send_signal_after_draw(text_buffer.signal, SIGNAL_TEXTBUFFER_JUMP_TO_OFFSET);
                return RustCompilerEvent::SelectMessage {path: dm.path.clone()}
            }
        }
        RustCompilerEvent::None
    }
    
    pub fn draw_rust_compiler(&mut self, cx: &mut Cx) {
        if let Err(_) = self.view.begin_view(cx, Layout::default()) {
            return
        }
        
        let mut counter = 0;
        for dm in &mut self._draw_messages {
            self.item_bg.color = dm.animator.last_color("bg.color");
            
            let bg_inst = self.item_bg.begin_quad(cx, &Layout {
                width: Bounds::Fill,
                height: Bounds::Compute, //::Fix(self.row_height),
                padding: Padding {l: 2., t: 3., b: 2., r: 0.},
                line_wrap: LineWrap::NewLine,
                ..Default::default()
            });
            
            match dm.level {
                TextBufferMessageLevel::Error => {
                    self.code_icon.draw_icon_walk(cx, CodeIconType::Error);
                },
                TextBufferMessageLevel::Warning => {
                    self.code_icon.draw_icon_walk(cx, CodeIconType::Warning);
                },
                TextBufferMessageLevel::Log => {
                    self.code_icon.draw_icon_walk(cx, CodeIconType::Ok);
                }
            }
            
            self.text.color = self.path_color;
            self.text.draw_text(cx, &format!("{}:{} - ", dm.path, dm.row));
            let walk = cx.get_rel_turtle_walk();
            cx.set_turtle_padding(Padding {l: walk.x, t: 3., b: 2., r: 0.});
            self.text.color = self.message_color;
            self.text.draw_text(cx, &format!("{}", dm.body));
            
            for line in &dm.more_lines {
                self.text.color = self.path_color;
                self.text.draw_text(cx, ".  ");
                self.text.color = self.message_color;
                self.text.draw_text(cx, line);
            }
            
            let bg_area = self.item_bg.end_quad(cx, &bg_inst);
            dm.animator.update_area_refs(cx, bg_area);
            
            cx.turtle_new_line();
            
            counter += 1;
            
            
        }
        
        let bg_even = cx.color("bg_selected");
        let bg_odd = cx.color("bg_odd");
        
        self.item_bg.color = if counter & 1 == 0 {bg_even}else {bg_odd};
        let bg_inst = self.item_bg.begin_quad(cx, &Layout {
            width: Bounds::Fill,
            height: Bounds::Compute, //Bounds::Fix(self.row_height),
            padding: Padding {l: 2., t: 3., b: 2., r: 0.},
            ..Default::default()
        });
        if self._rustc_done == true {
            self.text.color = self.path_color;
            match self._rustc_build_stages {
                BuildStage::NotRunning => {
                    self.code_icon.draw_icon_walk(cx, CodeIconType::Ok);
                    self.text.draw_text(cx, "Done");
                }
                BuildStage::Building => {
                    if self._run_when_done {
                        self.code_icon.draw_icon_walk(cx, CodeIconType::Ok);
                        self.text.draw_text(cx, "Running when ready");
                    }
                    else {
                        self.code_icon.draw_icon_walk(cx, CodeIconType::Ok);
                        self.text.draw_text(cx, "Building");
                    }
                },
                BuildStage::Complete => {
                    if !self._program_running {
                        self.code_icon.draw_icon_walk(cx, CodeIconType::Ok);
                        self.text.draw_text(cx, "Press F9 to run");
                    }
                    else if self._draw_messages.len() == 0 {
                        self.code_icon.draw_icon_walk(cx, CodeIconType::Ok);
                        self.text.draw_text(cx, "Application running");
                    }
                }
            };
        }
        else {
            self.code_icon.draw_icon_walk(cx, CodeIconType::Wait);
            self.text.color = self.path_color;
            self.text.draw_text(cx, &format!("Checking({})", self._rustc_artifacts.len()));
        }
        self.item_bg.end_quad(cx, &bg_inst);
        cx.turtle_new_line();
        counter += 1;
        
        // draw filler nodes
        
        let view_total = cx.get_turtle_bounds();
        let rect_now = cx.get_turtle_rect();
        let mut y = view_total.y;
        while y < rect_now.h {
            self.item_bg.color = if counter & 1 == 0 {bg_even}else {bg_odd};
            self.item_bg.draw_quad_walk(cx, Bounds::Fill, Bounds::Fix((rect_now.h - y).min(self.row_height)), Margin::zero());
            cx.turtle_new_line();
            y += self.row_height;
            counter += 1;
        }
        
        self.view.end_view(cx);
        
        if self._messages_updated {
            self._messages_updated = false;
            // scroll to bottom
            
        }
    }
    
    pub fn start_rust_builder(&mut self) {
        if let Some(child) = &mut self._build_child {
            let _ = child.kill();
        }
        if let Some(child) = &mut self._run_child {
            let _ = child.kill();
        }
        
        // start a release build
        self._rustc_build_stages = BuildStage::Building;
        
        let mut _child = spawn_process_command("cargo",&["build", "--release","--message-format=json"],"./edit_repo");
        
        if let Err(_) = _child {
            return;
        }
        
        let mut child = _child.unwrap(); 
        
        let mut stdout = child.stdout.take().unwrap();
        let signal = self._check_signal;
        std::thread::spawn(move || {
            loop {
                let mut data = vec![0; 4096];
                let n_bytes_read = stdout.read(&mut data).expect("cannot read");
                data.truncate(n_bytes_read);
                if n_bytes_read == 0 {
                    Cx::send_signal(signal, SIGNAL_BUILD_COMPLETE);
                    return
                }
            }
        });
        self._build_child = Some(child);
    }
    
    pub fn run_program(&mut self) {
        self._run_when_done = false;
        self._program_running = true;
        if let Some(child) = &mut self._run_child {
            let _ = child.kill();
        }
        
        let mut _child = spawn_process_command("cargo",&["run", "--release"],"./edit_repo");
        
        let mut child = _child.unwrap();
        
        let stdout_opt = child.stdout.take();
        
        if let None = stdout_opt {
            return;
        }
        
        let mut stdout = stdout_opt.unwrap();
        
        let (tx, rx) = mpsc::channel();
        let signal = self._check_signal;
        let thread = std::thread::spawn(move || {
            loop {
                let mut data = vec![0; 4096];
                let n_bytes_read = stdout.read(&mut data).expect("cannot read");
                data.truncate(n_bytes_read);
                let _ = tx.send(data);
                Cx::send_signal(signal, SIGNAL_RUN_OUTPUT);
                if n_bytes_read == 0 {
                    return
                }
            }
        });
        self._rx = Some(rx);
        self._thread = Some(thread);
        self._run_child = Some(child);
    }
    
    pub fn restart_rust_checker(&mut self, cx: &mut Cx, text_buffers: &mut TextBuffers) {
        self._data.truncate(0);
        self._rustc_messages.truncate(0);
        self._rustc_artifacts.truncate(0);
        self._draw_messages.truncate(0);
        self._rustc_done = false;
        self._rustc_build_stages = BuildStage::NotRunning;
        self._data.push(String::new());
        self.clear_textbuffer_messages(cx, text_buffers);
        
        if let Some(child) = &mut self._check_child {
            let _ = child.kill();
        }
        
        if let Some(child) = &mut self._build_child {
            let _ = child.kill();
        }

        let mut _child = spawn_process_command("cargo",&["check", "--message-format=json"],"./edit_repo");
        
        if let Err(_) = _child {
            return;
        }
        
        let mut child = _child.unwrap();
        
        let stdout_opt = child.stdout.take();
        
        if let None = stdout_opt {
            return;
        }
        
        let mut stdout = stdout_opt.unwrap();
        
        let (tx, rx) = mpsc::channel();
        let signal = self._check_signal;
        let thread = std::thread::spawn(move || {
            loop {
                let mut data = vec![0; 4096];
                let n_bytes_read = stdout.read(&mut data).expect("cannot read");
                data.truncate(n_bytes_read);
                let _ = tx.send(data);
                Cx::send_signal(signal, SIGNAL_RUST_CHECKER);
                if n_bytes_read == 0 {
                    return
                }
            }
        });
        self._rx = Some(rx);
        self._thread = Some(thread);
        self._check_child = Some(child);
    }
    
    pub fn process_compiler_messages(&mut self, cx: &mut Cx, datas: Vec<Vec<u8>>) {
        for data in datas {
            if data.len() == 0 { // last event
                self._rustc_done = true;
                // the check is done, do we have any errors? ifnot start a release build
                let mut has_errors = false;
                for dm in &self._draw_messages {
                    if dm.level == TextBufferMessageLevel::Error {
                        has_errors = true;
                        break;
                    }
                }
                if !has_errors { // start release build
                    self.start_rust_builder();
                }
                self.view.redraw_view_area(cx);
            }
            else {
                for ch in data {
                    if ch == '\n' as u8 {
                        // parse it
                        let line = self._data.last_mut().unwrap();
                        // parse the line
                        if line.contains("\"reason\":\"compiler-artifact\"") {
                            let parsed: Result<RustcCompilerArtifact> = serde_json::from_str(line);
                            match parsed {
                                Err(err) => println!("JSON PARSE ERROR {:?} {}", err, line),
                                Ok(parsed) => {
                                    self._rustc_artifacts.push(parsed);
                                }
                            }
                            self.view.redraw_view_area(cx);
                        }
                        else if line.contains("\"reason\":\"compiler-message\"") {
                            let parsed: Result<RustcCompilerMessage> = serde_json::from_str(line);
                            match parsed {
                                Err(err) => println!("JSON PARSE ERROR {:?} {}", err, line),
                                Ok(parsed) => {
                                    let spans = &parsed.message.spans;
                                    if spans.len() > 0 {
                                        for i in 0..spans.len() {
                                            let mut span = spans[i].clone();
                                            if !span.is_primary {
                                                continue
                                            }
                                            let mut more_lines = vec![];
                                            if let Some(label) = span.label {
                                                more_lines.push(label);
                                            }
                                            // if we have children fo process
                                            for child in &parsed.message.children {
                                                more_lines.push(child.message.clone());
                                            }
                                            //span.file_name = format!("/{}",span.file_name);
                                            span.level = Some(parsed.message.level.clone());
                                            self._draw_messages.push(RustDrawMessage {
                                                hit_state: HitState {..Default::default()},
                                                animator: Animator::new(Self::get_default_anim(cx, self._draw_messages.len(), false)),
                                                is_selected: false,
                                                path: span.file_name,
                                                row: span.line_start as usize,
                                                col: span.column_start as usize,
                                                tail: span.byte_start as usize,
                                                head: span.byte_end as usize,
                                                body: parsed.message.message.clone(),
                                                more_lines: more_lines,
                                                level: match parsed.message.level.as_ref() {
                                                    "warning" => TextBufferMessageLevel::Warning,
                                                    "error" => TextBufferMessageLevel::Error,
                                                    _ => TextBufferMessageLevel::Warning
                                                }
                                            });
                                        }
                                    }
                                    self._rustc_messages.push(parsed);
                                }
                            }
                            self.view.redraw_view_area(cx);
                        }
                        self._data.push(String::new());
                    }
                    else {
                        self._data.last_mut().unwrap().push(ch as char);
                    }
                }
            }
        }
    }
    
    pub fn process_run_messages(&mut self, cx: &mut Cx, datas: Vec<Vec<u8>>) {
        for data in datas {
            if data.len() == 0 { // last event
                self._program_running = false;
                self.view.redraw_view_area(cx);
            }
            else {
                for ch in data {
                    if ch == '\n' as u8 {
                        // parse it
                        let line = self._data.last_mut().unwrap();
                        // lets parse line
                        let mut tok = LineTokenizer::new(&line);
                        let mut path = String::new();
                        let mut row_str = String::new();
                        let mut col_str = String::new();
                        let mut body = String::new();
                        if tok.next == '[' {
                            tok.advance();
                            while tok.next != ':' && tok.next != '\0' {
                                path.push(tok.next);
                                tok.advance();
                            }
                            tok.advance();
                            while tok.next != ':' && tok.next != '\0' {
                                row_str.push(tok.next);
                                tok.advance();
                            }
                            tok.advance();
                            while tok.next != ']' && tok.next != '\0' {
                                col_str.push(tok.next);
                                tok.advance();
                            }
                            tok.advance();
                            tok.advance();
                            while tok.next != '\0' {
                                body.push(tok.next);
                                tok.advance();
                            }
                        }
                        else {
                            body = line.clone();
                        }
                        let row = if let Ok(row) = row_str.parse::<u32>() {row as usize}else {0};
                        let col = if let Ok(col) = col_str.parse::<u32>() {col as usize}else {0};
                        
                        self._draw_messages.push(RustDrawMessage {
                            hit_state: HitState {..Default::default()},
                            animator: Animator::new(Self::get_default_anim(cx, self._draw_messages.len(), false)),
                            is_selected: false,
                            more_lines: Vec::new(),
                            path: path,
                            row: row,
                            col: col,
                            tail: 0,
                            head: 0,
                            body: body,
                            level: TextBufferMessageLevel::Log
                        });
                        self._messages_updated = true;
                        self.view.redraw_view_area(cx);
                        self._data.push(String::new());
                    }
                    else {
                        self._data.last_mut().unwrap().push(ch as char);
                    }
                }
            }
        }
    }
}



#[derive(Clone, Deserialize, Default)]
pub struct RustcTarget {
    kind: Vec<String>,
    crate_types: Vec<String>,
    name: String,
    src_path: String,
    edition: String
}

#[derive(Clone, Deserialize, Default)]
pub struct RustcText {
    text: String,
    highlight_start: u32,
    highlight_end: u32
}

#[derive(Clone, Deserialize, Default)]
pub struct RustcSpan {
    file_name: String,
    byte_start: u32,
    byte_end: u32,
    line_start: u32,
    line_end: u32,
    column_start: u32,
    column_end: u32,
    is_primary: bool,
    text: Vec<RustcText>,
    label: Option<String>,
    suggested_replacement: Option<String>,
    sugggested_applicability: Option<String>,
    expansion: Option<Box<RustcExpansion>>,
    level: Option<String>
}

#[derive(Clone, Deserialize, Default)]
pub struct RustcExpansion {
    span: Option<RustcSpan>,
    macro_decl_name: String,
    def_site_span: Option<RustcSpan>
}

#[derive(Clone, Deserialize, Default)]
pub struct RustcCode {
    code: String,
    explanation: Option<String>
}

#[derive(Clone, Deserialize, Default)]
pub struct RustcMessage {
    message: String,
    code: Option<RustcCode>,
    level: String,
    spans: Vec<RustcSpan>,
    children: Vec<RustcMessage>,
    rendered: Option<String>
}

#[derive(Clone, Deserialize, Default)]
pub struct RustcProfile {
    opt_level: String,
    debuginfo: Option<u32>,
    debug_assertions: bool,
    overflow_checks: bool,
    test: bool
}

#[derive(Clone, Deserialize, Default)]
pub struct RustcCompilerMessage {
    reason: String,
    package_id: String,
    target: RustcTarget,
    message: RustcMessage
}

#[derive(Clone, Deserialize, Default)]
pub struct RustcCompilerArtifact {
    reason: String,
    package_id: String,
    target: RustcTarget,
    profile: RustcProfile,
    features: Vec<String>,
    filenames: Vec<String>,
    executable: Option<String>,
    fresh: bool
}
