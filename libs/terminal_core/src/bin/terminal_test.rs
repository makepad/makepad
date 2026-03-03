use makepad_terminal_core::Terminal;

fn main() {
    let mut term = Terminal::new(80, 24);
    
    // Simulate TUI startup WITH custom scroll region
    term.process_bytes(b"\x1b[1;20r"); // Scroll region 1..20
    term.process_bytes(b"\x1b[H\x1b[2J"); // Clear screen
    
    // Draw some TUI content
    term.process_bytes(b"HEADER\r\n");
    term.process_bytes(b"Line 1\r\n");
    term.process_bytes(b"Line 2\r\n");
    
    // Move cursor to bottom
    term.process_bytes(b"\x1b[24;1H");
    term.process_bytes(b"FOOTER");
    
    // Resize down
    term.resize(80, 20);
    
    println!("Scrollback:");
    for i in 0..term.screen().scrollback_len() {
        let row = term.screen().scrollback()[i].as_slice();
        let text: String = row.iter().map(|c| c.codepoint).collect();
        if text.trim().len() > 0 {
            println!("{:2}: '{}'", i, text.trim());
        }
    }
}
