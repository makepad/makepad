use std::{env, io, str, time::Duration, thread};

fn main() {
    // Get the path to the serial port from the command line.
    let path = env::args().nth(1).unwrap();
    // Set up the serial port.
    //
    // NOTE: There doesn't appear to be a way to set the timeout to infinity, so we set it to a
    // large value instead.
    let mut send_port = serialport::new(path, 115_200)
        .timeout(Duration::from_millis(1000000))
        .open()
        .unwrap();
    let mut recv_port = send_port.try_clone().unwrap();
    // Read from the serial port and print to the terminal
    thread::spawn(move || {
        loop {
            let mut buf = [0; 256];
            let len = recv_port.read(&mut buf).unwrap();
            print!("{}", str::from_utf8(&buf[..len]).unwrap());
        }
    });
    // Read from the terminal and send to the serial port
    loop {
        let mut message = String::new();
        io::stdin().read_line(&mut message).unwrap();
        send_port.write_all(message.as_bytes()).unwrap();
    }
}
