use makepad_terminal_core::Terminal;

fn main() {
    let mut terminal = Terminal::new(80, 24);
    // Fill the screen and generate scrollback
    for i in 0..30 {
        let s = format!("Line {}\n", i);
        terminal.process_bytes(s.as_bytes());
    }
    println!("Cursor Y before resize: {}", terminal.screen().cursor.y);
    println!("Scrollback len before resize: {}", terminal.screen().scrollback_len());
    
    // Resize to 30
    terminal.resize(80, 30);
    
    println!("Cursor Y after resize: {}", terminal.screen().cursor.y);
    println!("Scrollback len after resize: {}", terminal.screen().scrollback_len());
}
