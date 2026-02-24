use makepad_widgets::*;
use crate::makepad_widgets::makepad_platform::script::net::{
    socket_stream_pause_current, socket_stream_poll, socket_stream_send_bytes, SocketStreamPoll,
};
use crate::makepad_widgets::makepad_script::{
    script_err_io, script_err_limit, script_err_type_mismatch, script_err_unexpected,
    ScriptArrayStorage,
};
use std::cell::RefCell;
use std::collections::HashMap;
use std::net::UdpSocket;

#[derive(Script, ScriptHook)]
struct SocketMdcResponse {
    #[live]
    display_id: f64,
    #[live]
    command_id: f64,
    #[live]
    ack: bool,
    #[live]
    payload: Vec<u8>,
}

thread_local! {
    static SOCKET_MDC_BUFFERS: RefCell<HashMap<ScriptHandle, Vec<u8>>> = RefCell::new(HashMap::new());
}

fn clear_mdc_buffer(handle: ScriptHandle) {
    SOCKET_MDC_BUFFERS.with(|buffers| {
        buffers.borrow_mut().remove(&handle);
    });
}

fn parse_mdc_response_frame(buffer: &mut Vec<u8>) -> Result<Option<(u8, u8, bool, Vec<u8>)>, String> {
    loop {
        while !buffer.is_empty() && buffer[0] != 0xAA {
            buffer.remove(0);
        }
        if buffer.len() < 2 {
            return Ok(None);
        }
        if buffer[1] != 0xFF {
            buffer.remove(0);
            continue;
        }
        if buffer.len() < 5 {
            return Ok(None);
        }

        let display_id = buffer[2];
        let length = buffer[3] as usize;
        if length < 2 {
            buffer.remove(0);
            continue;
        }
        let frame_len = 5 + length;
        if buffer.len() < frame_len {
            return Ok(None);
        }

        let checksum = buffer[frame_len - 1];
        let checksum_calc = (buffer[1..frame_len - 1]
            .iter()
            .fold(0u32, |sum, byte| sum + *byte as u32)
            % 256) as u8;
        if checksum != checksum_calc {
            buffer.remove(0);
            continue;
        }

        let ack_or_nak = buffer[4];
        let command_id = buffer[5];
        let payload_len = length - 2;
        let payload = buffer[6..6 + payload_len].to_vec();
        buffer.drain(0..frame_len);

        if ack_or_nak != 0x41 && ack_or_nak != 0x4E {
            continue;
        }
        return Ok(Some((display_id, command_id, ack_or_nak == 0x41, payload)));
    }
}

fn build_mdc_frame(command_id: u8, display_id: u8, data: &[u8]) -> Result<Vec<u8>, String> {
    if data.len() > 255 {
        return Err("mdc data exceeds 255 bytes".to_string());
    }
    let mut payload = Vec::with_capacity(3 + data.len());
    payload.push(command_id);
    payload.push(display_id);
    payload.push(data.len() as u8);
    payload.extend_from_slice(data);
    let checksum = (payload
        .iter()
        .fold(0u32, |sum, byte| sum + *byte as u32)
        % 256) as u8;
    let mut frame = Vec::with_capacity(1 + payload.len() + 1);
    frame.push(0xAA);
    frame.extend_from_slice(&payload);
    frame.push(checksum);
    Ok(frame)
}

fn parse_mac_to_bytes(mac: &str) -> Result<[u8; 6], String> {
    let cleaned: String = mac.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    if cleaned.len() != 12 {
        return Err("MAC address must contain exactly 12 hex digits".to_string());
    }
    let mut out = [0u8; 6];
    for i in 0..6 {
        let byte_str = &cleaned[i * 2..i * 2 + 2];
        out[i] = u8::from_str_radix(byte_str, 16).map_err(|_| format!("invalid MAC byte {byte_str}"))?;
    }
    Ok(out)
}

fn script_value_to_bytes(vm: &mut ScriptVm, value: ScriptValue) -> Result<Vec<u8>, String> {
    if value.is_string_like() {
        return vm
            .string_with(value, |_vm, s| s.as_bytes().to_vec())
            .ok_or_else(|| "invalid string value".to_string());
    }
    let Some(array) = value.as_array() else {
        return Err("expected string or byte array".to_string());
    };
    match vm.bx.heap.array_storage(array) {
        ScriptArrayStorage::U8(v) => Ok(v.clone()),
        ScriptArrayStorage::U16(v) => Ok(v.iter().map(|b| *b as u8).collect()),
        ScriptArrayStorage::U32(v) => Ok(v.iter().map(|b| *b as u8).collect()),
        ScriptArrayStorage::F32(v) => Ok(v.iter().map(|b| *b as u8).collect()),
        ScriptArrayStorage::ScriptValue(v) => {
            let mut out = Vec::with_capacity(v.len());
            for value in v {
                let Some(num) = value.as_f64() else {
                    return Err("byte array values must be numbers".to_string());
                };
                if !(0.0..=255.0).contains(&num) {
                    return Err("byte array values must be in 0..255".to_string());
                }
                out.push(num as u8);
            }
            Ok(out)
        }
    }
}

pub fn register_socket_extensions(vm: &mut ScriptVm) {
    let net = vm.module(id_lut!(net));
    set_script_value_to_api!(vm, net.SocketMdcResponse);

    let socket_stream_type = vm.handle_type(id_lut!(socket_stream));

    vm.add_handle_method(
        socket_stream_type,
        id_lut!(next_mdc),
        script_args_def!(),
        move |vm, args| {
            let Some(handle) = script_value!(vm, args.self).as_handle() else {
                return script_err_unexpected!(vm.trap(), "invalid socket_stream state");
            };
            loop {
                let parsed = SOCKET_MDC_BUFFERS.with(|buffers| {
                    let mut buffers = buffers.borrow_mut();
                    let buffer = buffers.entry(handle).or_default();
                    parse_mdc_response_frame(buffer)
                });
                match parsed {
                    Ok(Some((display_id, command_id, ack, payload))) => {
                        return SocketMdcResponse {
                            display_id: display_id as f64,
                            command_id: command_id as f64,
                            ack,
                            payload,
                        }
                        .script_to_value(vm);
                    }
                    Ok(None) => {}
                    Err(err) => {
                        clear_mdc_buffer(handle);
                        return script_err_io!(vm.trap(), "{err}");
                    }
                }

                match socket_stream_poll(vm, handle) {
                    SocketStreamPoll::Data(chunk) => {
                        SOCKET_MDC_BUFFERS.with(|buffers| {
                            let mut buffers = buffers.borrow_mut();
                            let buffer = buffers.entry(handle).or_default();
                            buffer.extend_from_slice(&chunk);
                        });
                    }
                    SocketStreamPoll::Closed(Some(err)) => {
                        clear_mdc_buffer(handle);
                        return script_err_io!(vm.trap(), "{err}");
                    }
                    SocketStreamPoll::Closed(None) => {
                        clear_mdc_buffer(handle);
                        return NIL;
                    }
                    SocketStreamPoll::Pause => {
                        if let Err(err) = socket_stream_pause_current(vm, handle) {
                            clear_mdc_buffer(handle);
                            return script_err_unexpected!(vm.trap(), "{err}");
                        }
                        return NIL;
                    }
                    SocketStreamPoll::TooManyPaused => {
                        return script_err_limit!(vm.trap(), "too many paused socket MDC reads");
                    }
                    SocketStreamPoll::InvalidHandle => {
                        clear_mdc_buffer(handle);
                        return script_err_unexpected!(vm.trap(), "invalid socket_stream state");
                    }
                }
            }
        },
    );

    vm.add_handle_method(
        socket_stream_type,
        id_lut!(send_mdc_command),
        script_args_def!(command_id = NIL, display_id = NIL, data = NIL),
        move |vm, args| {
            let Some(handle) = script_value!(vm, args.self).as_handle() else {
                return script_err_unexpected!(vm.trap(), "invalid socket_stream state");
            };
            let command_id = script_value!(vm, args.command_id).as_f64().unwrap_or(-1.0);
            let display_id = script_value!(vm, args.display_id).as_f64().unwrap_or(-1.0);
            if !(0.0..=255.0).contains(&command_id) || !(0.0..=255.0).contains(&display_id) {
                return script_err_type_mismatch!(
                    vm.trap(),
                    "send_mdc_command expects command_id and display_id in 0..255"
                );
            }
            let data = match script_value_to_bytes(vm, script_value!(vm, args.data)) {
                Ok(data) => data,
                Err(err) => return script_err_type_mismatch!(vm.trap(), "{err}"),
            };
            let frame = match build_mdc_frame(command_id as u8, display_id as u8, &data) {
                Ok(frame) => frame,
                Err(err) => return script_err_io!(vm.trap(), "{err}"),
            };
            let frame_len = frame.len() as f64;
            if let Err(err) = socket_stream_send_bytes(vm, handle, frame) {
                return script_err_io!(vm.trap(), "{err}");
            }
            frame_len.into()
        },
    );

    vm.add_handle_method(
        socket_stream_type,
        id_lut!(send_mdc_set_content_download),
        script_args_def!(url = NIL, display_id = NIL),
        move |vm, args| {
            let Some(handle) = script_value!(vm, args.self).as_handle() else {
                return script_err_unexpected!(vm.trap(), "invalid socket_stream state");
            };
            let display_id = script_value!(vm, args.display_id).as_f64().unwrap_or(0.0);
            if !(0.0..=255.0).contains(&display_id) {
                return script_err_type_mismatch!(vm.trap(), "display_id must be in 0..255");
            }
            let url = script_value!(vm, args.url);
            if !url.is_string_like() {
                return script_err_type_mismatch!(
                    vm.trap(),
                    "send_mdc_set_content_download expects a URL string"
                );
            }
            let Some(url_bytes) = vm.string_with(url, |_vm, s| s.as_bytes().to_vec()) else {
                return script_err_type_mismatch!(
                    vm.trap(),
                    "send_mdc_set_content_download expects a URL string"
                );
            };
            if url_bytes.len() > 255 {
                return script_err_io!(vm.trap(), "content URL exceeds 255 bytes");
            }
            let mut data = vec![0x53, 0x80, url_bytes.len() as u8];
            data.extend_from_slice(&url_bytes);
            let frame = match build_mdc_frame(0xC7, display_id as u8, &data) {
                Ok(frame) => frame,
                Err(err) => return script_err_io!(vm.trap(), "{err}"),
            };
            let frame_len = frame.len() as f64;
            if let Err(err) = socket_stream_send_bytes(vm, handle, frame) {
                return script_err_io!(vm.trap(), "{err}");
            }
            frame_len.into()
        },
    );

    vm.add_method(
        net,
        id_lut!(wake_on_lan),
        script_args_def!(mac = NIL, host = NIL, port = NIL),
        move |vm, args| {
            let mac = script_value!(vm, args.mac);
            let host = script_value!(vm, args.host);
            let port = script_value!(vm, args.port);

            if !mac.is_string_like() {
                return script_err_type_mismatch!(vm.trap(), "wake_on_lan expects a MAC string");
            }
            let Some(mac_string) = vm.string_with(mac, |_vm, s| s.to_string()) else {
                return script_err_type_mismatch!(vm.trap(), "wake_on_lan expects a MAC string");
            };
            let mac_bytes = match parse_mac_to_bytes(&mac_string) {
                Ok(bytes) => bytes,
                Err(err) => return script_err_type_mismatch!(vm.trap(), "{err}"),
            };
            let host_string = if host.is_string_like() {
                vm.string_with(host, |_vm, s| s.to_string())
                    .unwrap_or_else(|| "255.255.255.255".to_string())
            } else {
                "255.255.255.255".to_string()
            };
            let port_value = if port.is_number() {
                port.as_f64().unwrap_or(9.0) as u16
            } else {
                9
            };

            let mut packet = vec![0xFF; 6];
            for _ in 0..16 {
                packet.extend_from_slice(&mac_bytes);
            }

            let socket = match UdpSocket::bind("0.0.0.0:0") {
                Ok(socket) => socket,
                Err(err) => return script_err_io!(vm.trap(), "wake_on_lan bind failed: {err}"),
            };
            let _ = socket.set_broadcast(host_string == "255.255.255.255");
            if let Err(err) = socket.send_to(&packet, format!("{}:{}", host_string, port_value)) {
                return script_err_io!(vm.trap(), "wake_on_lan send failed: {err}");
            }
            NIL
        },
    );
}

script_mod! {
    use mod.std
    use mod.net

    mod.edmx = {
        http_body: "
        <body onclick='document.documentElement.requestFullscreen()' ondblclick='location.reload()' style='margin:0;padding:20;background:#fff;color:#000;display:flex;height:100vh;overflow:hidden'>
        <b id='d' style='font:5vw sans-serif'></b>
        <script>
        u = location.origin + location.pathname + '?' + location.pathname.slice(1);
        f = () => {
            fetch(u)
            .then(r => r.ok ? r.text() : null)
            .then(t => { if (t !== null) d.innerText = t })
            .catch(e => 0)
            .finally(() => setTimeout(f, 1000));
        };
        f();
        </script>
        </body>
        "

        wait_for_socket_text: |socket, pass, fail_1, fail_2| {
            let text = ""
            loop{
                let chunk = socket.next_string()
                if chunk == nil return ""
                text += chunk
                if text.search(pass) >= 0 return pass
                if fail_1 != "" && text.search(fail_1) >= 0 return fail_1
                if fail_2 != "" && text.search(fail_2) >= 0 return fail_2
            }
        }

        build_content_json: |local_ip, local_port, file_id, file_size| {
            {
                schedule: [{
                    start_date: "1970-01-01"
                    stop_date: "2999-12-31"
                    start_time: "00:00:00"
                    contents: [{
                        image_url: "http://" + local_ip + ":" + local_port + "/image"
                        file_id: file_id
                        file_path: "/home/owner/content/Downloads/vxtplayer/epaper/mobile/contents/" + file_id + "/" + (file_id + ".png")
                        duration: 91326
                        file_size: "" + file_size
                        file_name: file_id + ".png"
                    }]
                }]
                name: "makepad-comfyui"
                version: 1
                create_time: "2026-01-01 00:00:00"
                id: file_id
                program_id: "com.samsung.ios.ePaper"
                content_type: "ImageContent"
                deploy_type: "MOBILE"
            }.to_json()
        }

        mdc_wait_for_command: |socket, command_id| {
            loop{
                let response = socket.next_mdc()
                if response == nil return nil
                if response.command_id == command_id return response
            }
        }

        upload_image: |display, content_url, sleep_seconds| {
            if display.mac != ""{
                net.wake_on_lan(display.mac)
                sleep_seconds(1)
            }

            let socket = net.socket_stream(net.SocketStreamOptions{
                host: display.ip
                port: "1515"
                use_tls: false
                ignore_ssl_cert: true
                read_timeout_ms: 250
                write_timeout_ms: 5000
            })

            let greeting = mod.edmx.wait_for_socket_text(socket, "MDCSTART<<TLS>>", "", "")
            if greeting == ""{
                socket.close()
                return {is_ok:false error:"EDMX error: missing TLS greeting"}
            }

            socket.start_tls(display.ip true)
            socket.write_string("123456")

            let auth_result = mod.edmx.wait_for_socket_text(
                socket,
                "MDCAUTH<<PASS>>",
                "MDCAUTH<<FAIL:0x01>>",
                "MDCAUTH<<FAIL:0x02>>"
            )
            if auth_result == "MDCAUTH<<FAIL:0x01>>"{
                socket.close()
                return {is_ok:false error:"EDMX auth failed: incorrect pin"}
            }
            if auth_result == "MDCAUTH<<FAIL:0x02>>"{
                socket.close()
                return {is_ok:false error:"EDMX auth failed: blocked"}
            }
            if auth_result != "MDCAUTH<<PASS>>"{
                socket.close()
                return {is_ok:false error:"EDMX auth failed: missing pass marker"}
            }

            socket.send_mdc_set_content_download(content_url 0)

            let response = mod.edmx.mdc_wait_for_command(socket, 199)
            socket.close()

            if response == nil{
                return {is_ok:false error:"EDMX error: missing MDC response"}
            }
            if !response.ack{
                return {is_ok:false error:"EDMX NAK payload: " + response.payload.to_string()}
            }
            {is_ok:true}
        }
    }
}
