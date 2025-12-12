use std::net::UdpSocket;

fn main() -> std::io::Result<()> {
    // Bind to all interfaces on port 5606 to receive broadcast packets
    let socket = UdpSocket::bind("127.0.0.1:5606")?;
    
    // Enable broadcast option (often required to receive broadcast packets on some OSes)
    socket.set_broadcast(true)?;
    
    let target = "10.0.0.131:5606";
    // Maximum UDP packet size is 65535 bytes
    let mut buf = [0u8; 65535];

    println!("Listening on 0.0.0.0:5606 and forwarding to {}", target);

    loop {
        match socket.recv_from(&mut buf) {
            Ok((amt, src)) => {
                // Forward the received data to the target address
                match socket.send_to(&buf[..amt], target) {
                    Ok(_) => {
                        // Optional: Print confirmation (can be removed for high traffic)
                        // println!("Forwarded {} bytes from {}", amt, src);
                    },
                    Err(e) => eprintln!("Failed to forward packet from {}: {}", src, e),
                }
            }
            Err(e) => eprintln!("Error receiving packet: {}", e),
        }
    }
}
