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

    loop {}
}