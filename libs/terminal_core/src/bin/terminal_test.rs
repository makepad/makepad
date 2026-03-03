use makepad_terminal_core::Terminal;

fn main() {
    let mut term = Terminal::new(80, 24);
    
    // Simulate TUI startup
    term.process_bytes(b"\x1b[1;24r"); // Scroll region 1..24
    term.process_bytes(b"\x1b[H\x1b[2J"); // Clear screen
    
    // Draw some TUI content
    term.process_bytes(b"HEADER\r\n");
    term.process_bytes(b"Line 1\r\n");
    term.process_bytes(b"Line 2\r\n");
    
    // Move cursor to rows - 2 (row 22, 0-based 21)
    term.process_bytes(b"\x1b[22;1H");
    term.process_bytes(b"PROMPT>");
    
    // Resize down by 4
    term.resize(80, 20);
    
    println!("Scrollback len: {}", term.screen().scrollback_len());
    println!("Cursor Y: {}", term.cursor().y);
}
