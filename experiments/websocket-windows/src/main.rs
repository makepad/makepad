
use windows::Networking::Sockets::IWebSocket;
use windows::Networking::Sockets::MessageWebSocket;
use windows::Networking::Sockets::MessageWebSocketMessageReceivedEventArgs;
use windows::Networking::Sockets::SocketMessageType;
use windows::Networking::Sockets::WebSocketClosedEventArgs;
use windows::Networking::*;
use windows::Foundation::*;
use windows::core::*;
use windows::Storage::Streams::*;

use std::{thread, time};

fn main() -> Result<()> {

    let mws = Sockets::MessageWebSocket::new()?;
    mws.MessageReceived(&TypedEventHandler::<MessageWebSocket, MessageWebSocketMessageReceivedEventArgs>::new(move |  _sender, args| {
        
        let msg = args.as_ref().unwrap();
        let messagetype = msg.MessageType()?;
        let dr =msg.GetDataReader()?;
        match messagetype {
            SocketMessageType::Binary => {
                println!("websocket: incoming binary data ->");
                while dr.UnconsumedBufferLength()?>0
                {
                    // readbytes can read full array in to [u8] - but this is convenient. 
                    let data = dr.ReadByte()?;
                    print!("0x{:02x} ", data);
                }
                println!();
                println!("websocket: <-- end data"); 
            },
            SocketMessageType::Utf8 => {
                    println!("websocket: incoming utf8 data ->"); 
                    let data = dr.ReadString(dr.UnconsumedBufferLength()?)?;
                    println!("{}", data);
                    println!("websocket: <-- end data"); 
            },                        
            _ => println!("other"),
        }

        Ok(())
    }))?;

    mws.Closed(&TypedEventHandler::<IWebSocket, WebSocketClosedEventArgs>::new(move |_sender, _args | {
        println!("Websocket closed by host!");
        Ok(())
    } ))?; 

    let host = windows::Foundation::Uri::CreateUri(h!("ws://localhost:8080"))? ;
    let operation = mws.ConnectAsync(&host)?;
    println!("get..");
    let connectionattempt = operation.get();
    if connectionattempt == Ok(())
    {
        let output_stream = mws.OutputStream()?;

        println!("Sending a string.");
        let out = DataWriter::CreateDataWriter(&output_stream).unwrap();
        mws.Control()?.SetMessageType( SocketMessageType::Utf8)?;
        out.WriteString(h!("Hello, world!"))?;
        out.StoreAsync()?.get()?;
        out.FlushAsync()?.get()?;
    
        println!("Sending something binary.");
        mws.Control()?.SetMessageType( SocketMessageType::Binary)?;
        out.WriteBytes(&[1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16])?;
        out.StoreAsync()?.get()?;
        out.FlushAsync()?.get()?;

        println!("Waiting 1 second to get echo results back.");
        let asecond = time::Duration::from_millis(1000);
        thread::sleep(asecond);


    }
    else
    {
        let error = connectionattempt.unwrap_err();
        println!("Error attempting to connect websocket: {}", error);
    };

    

  
    Ok(())
    
}