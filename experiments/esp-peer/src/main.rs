#![no_main]
#![no_std]

mod usb_serial_jtag;

use {
    crate::usb_serial_jtag::UsbSerialJtag,
    esp_backtrace as _,
    esp_hal::{
        clock::ClockControl, peripherals::Peripherals, prelude::*, systimer::SystemTimer, Rng,uart::*, IO, delay::*
    },
    esp_wifi::{esp_now, esp_now::EspNow, EspWifiInitFor},
};
use core::fmt::Write; // allows use to use the WriteLn! macro for easy printing

use esp_hal::uart::config::*;

#[entry]
fn main() -> ! {
    // Set up the USB Serial JTAG and ESP-NOW interfaces.
    let peripherals = Peripherals::take();
    UsbSerialJtag::init(peripherals.USB_DEVICE);
    let system = peripherals.SYSTEM.split();
    let clocks = ClockControl::max(system.clock_control).freeze();
    let inited = esp_wifi::initialize(
        EspWifiInitFor::Wifi,
        SystemTimer::new(peripherals.SYSTIMER).alarm0,
        Rng::new(peripherals.RNG),
        system.radio_clock_control,
        &clocks,
    )
    .unwrap();
    let mut esp_now = EspNow::new(&inited, peripherals.WIFI).unwrap();

    let mut bytes = [0; 1024];
    let mut len = 0;
    let mut discard = false;

    let config = Config {
        baudrate: 115200,
        data_bits: DataBits::DataBits8,
        parity: Parity::ParityNone,
        stop_bits: StopBits::STOP1,
    };
    
    let io = IO::new(peripherals.GPIO, peripherals.IO_MUX);
    let pins = TxRxPins::new_tx_rx(
        io.pins.gpio7.into_push_pull_output(),
        io.pins.gpio6.into_floating_input(),
    );
    let mut delay = Delay::new(&clocks);


    let mut serial1 = Uart::new_with_config(peripherals.UART1, config, Some(pins), &clocks);
    let mut counter:i32 = 1;
    serial1.write('\n' as u8).ok();
    loop {
        /*delay.delay_ms(500 as u32);
        serial1.write('M' as u8).ok();
        serial1.write('a' as u8).ok();
        serial1.write('k' as u8).ok();
        serial1.write('e' as u8).ok();
        delay.delay_ms(500 as u32);
        serial1.write('p' as u8).ok();
        serial1.write('a' as u8).ok();
        serial1.write('d' as u8).ok();
        serial1.write('\n' as u8).ok();
        */
        //serial1.write("haha".as_bytes()).ok();
        writeln!(serial1, "Loop {}", counter).unwrap();
        delay.delay_ms(100 as u32);
        // serial1.flush();
        //delay.delay_ms(500 as u32);

        counter = counter + 1;

        // Read bytes from the USB Serial JTAG interface.
        UsbSerialJtag::with(|usb_serial_jtag| {
            len += usb_serial_jtag.read_bytes(&mut bytes);
        });
        // Did we receive a newline character? If so, we've received a full message. Send all
        // bytes up to and including the newline over the ESP-NOW interface, unless the discard
        // flag was set below.
        if let Some(pos) = bytes[..len].iter().position(|b| *b == b'\n') {
            if !discard {
                let msg = &bytes[..pos + 1];
                serial1.write_str("Sending: ");
                for i in 0..msg.len(){
                    serial1.write(msg[i]);
                }
                serial1.write_str("\n");
                
                esp_now
                    .send(&esp_now::BROADCAST_ADDRESS, &bytes[..pos + 1])
                    .unwrap()
                    .wait()
                    .unwrap();
                delay.delay_ms(500 as u32);
            }
            // Copy the remaining bytes to the start of the buffer.
            bytes.copy_within(pos + 1..len, 0);
            len -= pos + 1;
            discard = false;
        }
        // Is the buffer full? If so, the message is too large. Discard all bytes read so far, and
        // set the discard flag so that all bytes are discarded until a newline character is
        // received.
        if len == bytes.len() {
            len = 0;
            discard = true;
        }
        // Read bytes from the ESP-NOW interface, and write them to the USB Serial JTAG interface.
        if let Some(data) = esp_now.receive() {
            UsbSerialJtag::with(|usb_serial_jtag| {
                usb_serial_jtag.write_bytes(&data.data[..data.len as usize]);
            })
        }
    }
}
