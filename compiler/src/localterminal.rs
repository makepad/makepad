use render::*; 
use editor::*;
use crate::terminal::*;

#[derive(Clone)]
pub struct LocalTerminal {
    pub terminal: Terminal,
    pub text_buffer: TextBuffer
}

impl Style for LocalTerminal {
    fn style(cx: &mut Cx) -> Self {
        let local_terminal = Self {
            terminal: Terminal::style(cx),
            text_buffer: TextBuffer::default(),
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
    pub fn handle_local_terminal(&mut self, cx: &mut Cx, event: &mut Event) -> TerminalEvent {
        let ce = self.terminal.handle_terminal(cx, event, &mut self.text_buffer);
        ce
    }
    
    pub fn draw_local_terminal(&mut self, cx: &mut Cx) {
        if let Err(_) = self.terminal.begin_terminal(cx, &mut self.text_buffer) {
            return
        }
        
        
        self.terminal.end_terminal(cx, &mut self.text_buffer);
    }
}