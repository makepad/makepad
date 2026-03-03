use makepad_terminal_core::Terminal;

fn main() {
    let mut terminal = Terminal::new(80, 24);
    for i in 0..30 {
        let s = format!("Line {}\n", i);
        terminal.process_bytes(s.as_bytes());
    }
    terminal.resize(80, 30);
    println!("Cursor Y after resize: {}", terminal.screen().cursor.y);
    println!("Scrollback len after resize: {}", terminal.screen().scrollback_len());
}
