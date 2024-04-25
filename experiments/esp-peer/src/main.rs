#![no_main]
#![no_std]

mod usb_serial_jtag;

use {
    crate::usb_serial_jtag::UsbSerialJtag,
    esp_backtrace as _,
    esp_hal::{
        clock::ClockControl, peripherals::Peripherals, prelude::*, systimer::SystemTimer, Rng,
    },
    esp_wifi::{esp_now, esp_now::EspNow, EspWifiInitFor},
};

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

    let mut bytes = [0; 250];
    let mut len = 0;
    let mut discard = false;
    loop {
        // Read bytes from the USB Serial JTAG interface.
        UsbSerialJtag::with(|usb_serial_jtag| {
            len += usb_serial_jtag.read_bytes(&mut bytes);
        });
        // Did we receive a newline character? If so, we've received a full message. Send all
        // bytes up to and including the newline over the ESP-NOW interface, unless the discard
        // flag was set below.
        if let Some(pos) = bytes[..len].iter().position(|b| *b == b'\n') {
            if !discard {
                esp_now
                    .send(&esp_now::BROADCAST_ADDRESS, &bytes[..pos + 1])
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
        // Read bytes from the ESP-NOW interface, and write them to the USB Serial JTAG interface.
        if let Some(data) = esp_now.receive() {
            UsbSerialJtag::with(|usb_serial_jtag| {
                usb_serial_jtag.write_bytes(&data.data[..data.len as usize]);
            })
        }
    }
}
