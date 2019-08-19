use render::*;
use crate::terminal::*;
use process::*;

pub struct LocalTerminal {
    pub terminal: Terminal,
    pub term_buffer: TermBuffer,
    pub process: Option<Process>
}

impl Clone for LocalTerminal {
    fn clone(&self) -> Self {
        LocalTerminal {
            terminal: self.terminal.clone(),
            term_buffer: self.term_buffer.clone(),
            process:None
        }
    }
}

impl Style for LocalTerminal {
    fn style(cx: &mut Cx) -> Self {
        let local_terminal = Self {
            terminal: Terminal::style(cx),
            term_buffer: TermBuffer::default(),
            process: None
        };
        //tab.animator.default = tab.anim_default(cx);
        local_terminal
    }
}

#[derive(Clone, PartialEq)]
pub enum LocalTerminalEvent {
    None,
    Change
}

impl LocalTerminal {
    pub fn start_terminal(&mut self, _cx: &mut Cx) {
        self.process = Some(Process::start())
        
    }
    
    pub fn handle_local_terminal(&mut self, cx: &mut Cx, event: &mut Event) -> TerminalEvent {
        let ce = self.terminal.handle_terminal(cx, event, &mut self.term_buffer);
        // if we are at construct?... open one
        ce
    }
    
    pub fn draw_local_terminal(&mut self, cx: &mut Cx) {
        if let Err(_) = self.terminal.begin_terminal(cx, &mut self.term_buffer) {
            return
        }
        
        self.terminal.end_terminal(cx, &mut self.term_buffer);
    }
}