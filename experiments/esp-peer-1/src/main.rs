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
        writeln!(usb_serial_jtag, "Started").unwrap();
    });

    loop {}
}