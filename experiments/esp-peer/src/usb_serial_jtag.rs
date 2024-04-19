use {
    core::{cell::RefCell, fmt},
    critical_section::Mutex,
    esp_hal::{
        interrupt,
        interrupt::Priority,
        peripheral::Peripheral,
        peripherals::{Interrupt, USB_DEVICE},
        prelude::*,
        usb_serial_jtag::UsbSerialJtag as UsbserialJtagInner,
    },
};

/// A USB Serial JTAG interface.
pub struct UsbSerialJtag<'a> {
    inner: UsbserialJtagInner<'a>,
    queue: Queue,
}

impl<'a> UsbSerialJtag<'a> {
    // Initialize the USB Serial JTAG interface.
    pub fn init(usb_device: impl Peripheral<P = USB_DEVICE> + 'static) {
        let mut inner = UsbserialJtagInner::new(usb_device);
        // Listen for receive packet interrupts.
        inner.listen_rx_packet_recv_interrupt();
        critical_section::with(|cs| {
            USB_SERIAL_JTAG.borrow_ref_mut(cs).replace(UsbSerialJtag {
                inner,
                queue: Queue::new(),
            })
        });
        // Enable the USB_DEVICE interrupt.
        interrupt::enable(Interrupt::USB_DEVICE, Priority::Priority1).unwrap();
    }

    /// Acquires a mutable reference to the USB Serial JTAG interface.
    /// 
    /// This will disable interrupts for the duration of this call.
    pub fn with(f: impl FnOnce(&mut UsbSerialJtag)) {
        critical_section::with(|cs| f(USB_SERIAL_JTAG.borrow_ref_mut(cs).as_mut().unwrap()));
    }

    /// Read bytes from the USB Serial JTAG interface into the provided buffer. Returns the number
    /// of bytes read.
    pub fn read_bytes(&mut self, bytes: &mut [u8]) -> usize {
        let mut count = 0;
        while count != bytes.len() && !self.queue.is_empty() {
            bytes[count] = self.queue.pop().unwrap();
            count += 1;
        }
        count
    }

    /// Write bytes to the USB Serial JTAG interface from the provided buffer.
    pub fn write_bytes(&mut self, bytes: &[u8]) {
        self.inner.write_bytes(bytes).unwrap();
    }
}

impl<'a> fmt::Write for UsbSerialJtag<'a> {
    fn write_str(&mut self, str: &str) -> fmt::Result {
        self.inner.write_str(str)
    }
}

struct Queue {
    data: [u8; 1024],
    head: usize,
    tail: usize,
}

impl Queue {
    const fn new() -> Self {
        Self {
            data: [0; 1024],
            head: 0,
            tail: 0,
        }
    }

    fn is_empty(&self) -> bool {
        self.head == self.tail
    }

    fn is_full(&self) -> bool {
        (self.tail + 1) % self.data.len() == self.head
    }

    fn push(&mut self, byte: u8) {
        self.data[self.tail] = byte;
        self.tail = (self.tail + 1) % self.data.len();
    }

    fn pop(&mut self) -> Option<u8> {
        if self.head == self.tail {
            None
        } else {
            let byte = self.data[self.head];
            self.head = (self.head + 1) % self.data.len();
            Some(byte)
        }
    }
}

static USB_SERIAL_JTAG: Mutex<RefCell<Option<UsbSerialJtag>>> = Mutex::new(RefCell::new(None));

// Handle the USB_DEVICE interrupt.
#[interrupt]
fn USB_DEVICE() {
    UsbSerialJtag::with(|usb_serial_jtag| {
        // Read bytes from the USB Serial JTAG interface and push them onto the queue
        // until either the queue is full or there are no more bytes to read.
        //
        // NOTE: I am unsure what happens if we don't read all bytes from the USB Serial JTAG
        // interface because the queue is full. Will they be buffered somewhere? Or will they
        // be lost?
        while !usb_serial_jtag.queue.is_full() {
            if let Result::Ok(byte) = usb_serial_jtag.inner.read_byte() {
                usb_serial_jtag.queue.push(byte);
            } else {
                break;
            }
        }
        // Reset the receive packet interrupt.
        usb_serial_jtag.inner.reset_rx_packet_recv_interrupt();
    });
}
