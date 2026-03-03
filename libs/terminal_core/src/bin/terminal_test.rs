use makepad_terminal_core::Terminal;

fn main() {
    let mut term = Terminal::new(80, 24);
    
    // Fill the screen exactly with long lines
    for i in 0..24 {
        term.process_bytes(format!("This is a very long line number {:02}\r\n", i).as_bytes());
    }
    
    // Add one more line so line 00 goes to scrollback
    term.process_bytes(b"This is line 24\r\n");
    
    // Position cursor somewhere
    term.process_bytes(b"\x1b[10;10H");
    
    term.resize(20, 24);
    term.resize(80, 24);
    
    println!("After resize wider (80 cols):");
    println!("Scrollback len: {}", term.screen().scrollback_len());
    for i in 0..term.screen().scrollback_len() {
        let row = term.screen().scrollback()[i].as_slice();
        let text: String = row.iter().map(|c| c.codepoint).collect();
        println!("SB {:2}: '{}'", i, text.trim_end());
    }
    for i in 0..24 {
        let row = term.screen().grid.row_slice(i);
        let text: String = row.iter().map(|c| c.codepoint).collect();
        println!("{:2}: '{}'", i, text.trim_end());
    }
}
