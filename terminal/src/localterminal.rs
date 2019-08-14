use render::*; 
use crate::terminal::*;

#[derive(Clone)]
pub struct LocalTerminal {
    pub terminal: Terminal,
    pub term_buffer: TermBuffer
}

impl Style for LocalTerminal {
    fn style(cx: &mut Cx) -> Self {
        let local_terminal = Self {
            terminal: Terminal::style(cx),
            term_buffer: TermBuffer::default(),
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
    pub fn start_terminal(&mut self, _cx: &mut Cx){
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