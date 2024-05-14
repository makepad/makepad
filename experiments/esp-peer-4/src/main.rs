#![no_main]
#![no_std]
#![allow(dead_code)]
#![allow(unused_imports)]

mod usb_serial_jtag;

use {
    crate::usb_serial_jtag::UsbSerialJtag,
    core::fmt::Write,
    esp_backtrace as _,
    esp_hal::{
        clock::ClockControl,
        delay::Delay,
        peripherals::Peripherals,
        prelude::*,
        systimer::SystemTimer,
        uart::{
            config::{Config, DataBits, Parity, StopBits},
            TxRxPins, Uart,
        },
        Rng, IO,
    },
    esp_wifi::{esp_now, esp_now::EspNow, EspWifiInitFor},
};

#[entry]
fn main() -> ! {
    // Set up the USB Serial JTAG interface.
    let peripherals = Peripherals::take();
    UsbSerialJtag::init(peripherals.USB_DEVICE);

    // Write a start message to the serial USB JTAG interface.
    UsbSerialJtag::with(|usb_serial_jtag| {
        writeln!(usb_serial_jtag, "ESP-32 started").unwrap();
    });

    // Set up the UART interface.
    let system = peripherals.SYSTEM.split();
    let clocks = ClockControl::max(system.clock_control).freeze();
    let io = IO::new(peripherals.GPIO, peripherals.IO_MUX);
    let pins = TxRxPins::new_tx_rx(
        io.pins.gpio7.into_push_pull_output(),
        io.pins.gpio6.into_floating_input(),
    );
    let mut uart = Uart::new_with_config(
        peripherals.UART1,
        Config {
            baudrate: 115200,
            data_bits: DataBits::DataBits8,
            parity: Parity::ParityNone,
            stop_bits: StopBits::STOP1,
        },
        Some(pins),
        &clocks,
    );

    // Write a start message to the UART interface.
    writeln!(uart, "ESP-32 started").unwrap();

    // Set up the ESP-NOW interface.
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

    let mut delay = Delay::new(&clocks);
    loop {
        // Avoid flooding the UART interface.
        delay.delay_ms(50 as u32);

        // Read bytes from the USB Serial JTAG interface.
        UsbSerialJtag::with(|usb_serial_jtag| {
            len += usb_serial_jtag.read_bytes(&mut bytes);
        });

        // Did we receive a newline character? If so, we've received a full message.
        if let Some(pos) = bytes[..len].iter().position(|b| *b == b'\n') {
            // Handle the message, unless it should be discarded.
            if !discard {
                let message = &bytes[..pos + 1];

                // Write the message to the UART interface.
                uart.write_str(">").ok();
                for &b in message {
                    uart.write(b).ok();
                }

                // Write the message to the ESP-NOW interface.
                esp_now
                    .send(&esp_now::BROADCAST_ADDRESS, message)
                    .unwrap()
                    .wait()
                    .unwrap();
            }

            // Copy the remaining bytes to the start of the buffer.
            bytes.copy_within(pos + 1..len, 0);
            len -= pos + 1;
            discard = false;
        }

        // Is the buffer full? If so, the message is too large. Discard all bytes read so far, and
        // set the discard flag so that the entire message will be discarded when a newline
        // character is received.
        if len == bytes.len() {
            len = 0;
            discard = true;
        }

        // Read a message from the ESP-NOW interface.
        if let Some(data) = esp_now.receive() {
            let message = &data.data[..data.len as usize];

            // Write the received message to the UART interface.
            uart.write_str("<").ok();
            for &b in message {
                uart.write(b).ok();
            }

            // Write the received message to the USB Serial JTAG interface.
            UsbSerialJtag::with(|usb_serial_jtag| {
                usb_serial_jtag.write_bytes(message);
            })
        }
    }
}
