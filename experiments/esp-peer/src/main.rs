#![no_main]
#![no_std]

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
    let peripherals = Peripherals::take();
    let system = peripherals.SYSTEM.split();
    let clocks = ClockControl::max(system.clock_control).freeze();

    // Set up the USB Serial JTAG interface.
    UsbSerialJtag::init(peripherals.USB_DEVICE);

    // Set up the UART interface.
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

    uart.write(b'\n').ok();
    writeln!(uart, "Started").unwrap();

    let mut bytes = [0; 1024];
    let mut len = 0;
    let mut discard = false;

    let mut delay = Delay::new(&clocks);
    loop {
        delay.delay_ms(50 as u32);

        // Read bytes from the USB Serial JTAG interface.
        UsbSerialJtag::with(|usb_serial_jtag| {
            len += usb_serial_jtag.read_bytes(&mut bytes);
        });

        // Did we receive a newline character? If so, we've received a full message. Send all
        // bytes up to and including the newline over the ESP-NOW interface, unless the discard
        // flag was set below.
        if let Some(pos) = bytes[..len].iter().position(|b| *b == b'\n') {
            if !discard {
                let bytes_to_be_sent = &bytes[..pos + 1];

                // Write the bytes to be sent to the UART interface.
                uart.write_str(">").ok();
                for &b in bytes_to_be_sent {
                    uart.write(b).ok();
                }

                // Write the bytes to be sent to the ESP-NOW interface.
                esp_now
                    .send(&esp_now::BROADCAST_ADDRESS, bytes_to_be_sent)
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
        // set the discard flag so that all bytes are discarded until a newline character is
        // received.
        if len == bytes.len() {
            len = 0;
            discard = true;
        }

        // Read bytes from the ESP-NOW interface.
        if let Some(msg) = esp_now.receive() {
            let received_bytes = &msg.data[..msg.len as usize];

            // Write the received bytes to the UART interface.
            uart.write_str("<").ok();
            for &b in received_bytes {
                uart.write(b).ok();
            }

            // Write the received bytes to the USB Serial JTAG interface.
            UsbSerialJtag::with(|usb_serial_jtag| {
                usb_serial_jtag.write_bytes(received_bytes);
            })
        }
    }
}
