use core::mem;
use core::ops::BitOr;

use rp_pico::hal as hal;

use cortex_m::delay::Delay;
use embedded_hal::digital::v2::OutputPin;
use embedded_hal::prelude::_embedded_hal_blocking_spi_Write;
use hal::gpio::{Pin, PinId, PushPullOutput};
use hal::spi::{Enabled, SpiDevice};
use hal::typelevel::NoneT;
use crate::font::Font;

#[repr(u8)]
#[allow(dead_code)]
pub enum Command {
    Nop = 0x00,
    Swreset = 0x01,
    Rddid = 0x04,
    Rddst = 0x09,
    Slpin = 0x10,
    Slpout = 0x11,
    Ptlon = 0x12,
    Noron = 0x13,
    Invoff = 0x20,
    Invon = 0x21,
    Dispoff = 0x28,
    Dispon = 0x29,
    Caset = 0x2A,
    Raset = 0x2B,
    Ramwr = 0x2C,
    Ramrd = 0x2E,
    Ptlar = 0x30,
    Vscrdef = 0x33,
    Colmod = 0x3A,
    Madctl = 0x36,
    Vscsad = 0x37,
}

#[repr(u8)]
#[allow(dead_code)]
pub enum Madctl {
    MY = 0x80,
    MX = 0x40,
    MV = 0x20,
    ML = 0x10,
    BGR = 0x08,
    MH = 0x04,
    RGB = 0x00,
}

#[repr(u8)]
#[allow(dead_code)]
pub enum ColorMode {
    ColorMode65k = 0x50,
    ColorMode262k = 0x60,
    ColorMode12bit = 0x03,
    ColorMode16bit = 0x05,
    ColorMode18bit = 0x06,
    ColorMode16m = 0x07,
}

#[repr(u8)]
#[allow(dead_code)]
pub enum Rotation {
    Portrait = 0,
    Landscape = 0x60,
    InvertedPortrait = 0xC0,
    InvertedLandscape = 0xA0,
}

impl BitOr for ColorMode {
    type Output = u8;
    fn bitor(self, rhs: Self) -> Self::Output {
        self as u8 | rhs as u8
    }
}

fn abs(x: i16) -> i16 {
    if x < 0 {
        -x
    } else {
        x
    }
}

/// OptionalOutputPin is used to implement some optional output pins.
pub trait OptionalOutputPin {
    /// Set the output pin to the specified value.
    fn set(&mut self, value: bool);
    /// Return whether the output pin is none.
    fn is_none(&self) -> bool;
}

impl<L: PinId> OptionalOutputPin for Pin<L, PushPullOutput> {
    fn set(&mut self, value: bool) {
        if value {
            self.set_high().unwrap();
        } else {
            self.set_low().unwrap();
        }
    }

    fn is_none(&self) -> bool {
        false
    }
}

impl OptionalOutputPin for NoneT {
    fn set(&mut self, _: bool) {}
    fn is_none(&self) -> bool {
        true
    }
}

/// The ST7789 display driver.
pub struct ST7789Display<
    K: OptionalOutputPin,
    L: PinId,
    M: OptionalOutputPin,
    N: OptionalOutputPin,
    S: SpiDevice
> {
    /// Reset
    reset_pin: K,
    /// Data/Command
    dc_pin: Pin<L, PushPullOutput>,
    /// Chip select
    cs_pin: M,
    /// Backlight
    bl_pin: N,
    /// SPI
    spi: hal::spi::Spi<Enabled, S, 8>,
    /// the width of the display in pixels
    width: u16,
    /// the height of the display in pixels
    height: u16,
    colstart: u16,
    rowstart: u16,
}

const BUFFER_SIZE: u16 = 512;

#[allow(dead_code)]
impl<K: OptionalOutputPin, L: PinId, M: OptionalOutputPin, N: OptionalOutputPin, S: SpiDevice> ST7789Display<K, L, M, N, S> {
    /// Creates a new display driver.
    pub fn new(
        // Reset
        reset_pin: K,
        // Data/Command
        dc_pin: Pin<L, PushPullOutput>,
        // Chip select
        cs_pin: M,
        // Backlight
        bl_pin: N,
        // SPI
        spi: hal::spi::Spi<Enabled, S, 8>,
        width: u16,
        height: u16,
        rotation: Rotation,
        delay: &mut Delay,
        colstart: u16,
        rowstart: u16,
    ) -> Self
    {
        let mut i = Self {
            reset_pin,
            dc_pin,
            cs_pin,
            bl_pin,
            spi,
            width,
            height,
            colstart,
            rowstart
        };

        i.hard_reset(delay);
        i.soft_reset(delay);
        i.set_sleep_mode(false);
        delay.delay_ms(10);
        i.set_color_mode(ColorMode::ColorMode65k | ColorMode::ColorMode16bit);
        delay.delay_ms(50);
        i.set_rotation(rotation);
        i.set_inversion_mode(true);
        delay.delay_ms(10);
        i.send_command(Command::Noron);
        delay.delay_ms(10);
        i.bl_pin.set(true);
        i.fill(0);
        i.send_command(Command::Dispon);
        delay.delay_ms(500);
        i
    }

    /// Reset the display by resetting the reset pin.
    /// It will be called automatically when created.
    /// It is usually called before `soft_reset`.
    pub fn hard_reset(&mut self, delay: &mut Delay) {
        if self.reset_pin.is_none() {
            return;
        }
        self.cs_pin.set(false);
        self.reset_pin.set(true);
        delay.delay_ms(50);
        self.reset_pin.set(false);
        delay.delay_ms(50);
        self.reset_pin.set(true);
        delay.delay_ms(150);
        self.cs_pin.set(true);
    }

    /// Write Spi command to the display.
    pub fn send_command(&mut self, command: Command) {
        self.cs_pin.set(false);
        self.dc_pin.set_low().unwrap();
        self.spi.write(&[command as u8]).unwrap();
        self.cs_pin.set(true);
    }

    /// Write Spi data to the display.
    pub fn send_data(&mut self, data: &[u8]) {
        self.cs_pin.set(false);
        self.dc_pin.set_high().unwrap();
        self.spi.write(data).unwrap();
        self.cs_pin.set(true);
    }

    /// Reset by sending a software reset command.
    /// It will be called automatically when created.
    /// It is usually called after `hard_reset`.
    pub fn soft_reset(&mut self, delay: &mut Delay) {
        self.send_command(Command::Swreset);
        delay.delay_ms(150);
    }

    /// Set the display to sleep mode.
    pub fn set_sleep_mode(&mut self, value: bool) {
        if value {
            self.send_command(Command::Slpin);
        } else {
            self.send_command(Command::Slpout);
        }
    }

    /// Set the display to inversion mode.
    pub fn set_inversion_mode(&mut self, value: bool) {
        if value {
            self.send_command(Command::Invon);
        } else {
            self.send_command(Command::Invoff);
        }
    }

    /// Set the display to color mode.
    ///
    /// If the parameter is a single value, pass it like `ColorMode::ColorMode65k as u8`.
    ///
    /// If the parameter is two value, pass it like `ColorMode::ColorMode65k | ColorMode::ColorMode16bit`.
    pub fn set_color_mode(&mut self, mode: u8) {
        self.send_command(Command::Colmod);
        self.send_data(&[mode]);
    }

    /// Set the display to rotation mode.
    pub fn set_rotation(&mut self, rotation: Rotation) {
        self.send_command(Command::Madctl);
        self.send_data(&[rotation as u8]);
    }

    /// Select columns.
    fn set_columns(&mut self, start: u16, end: u16) {
        assert!(start <= end && end <= self.width);
        self.send_command(Command::Caset);
        let adjusts = start + self.colstart;
        let adjuste = end +  self.colstart;


        self.send_data(&[(adjusts >> 8) as u8, (adjusts & 0xff) as u8, (adjuste >> 8) as u8, (adjuste & 0xff) as u8]);
    }

    /// Select rows.
    fn set_rows(&mut self, start: u16, end: u16) {
        assert!(start <= end && end <= self.height);
        self.send_command(Command::Raset);
        let adjusts = start + self.rowstart;
        let adjuste = end +  self.rowstart;
        self.send_data(&[(adjusts >> 8) as u8, (adjusts & 0xff) as u8, (adjuste >> 8) as u8, (adjuste & 0xff) as u8]);
//        self.send_data(&[(start >> 8) as u8, (start & 0xff) as u8, (end >> 8) as u8, (end & 0xff) as u8]);
    }

    /// Select a window.
    pub fn set_window(&mut self, start_x: u16, start_y: u16, end_x: u16, end_y: u16) {
        self.set_columns(start_x , end_x);
        self.set_rows(start_y, end_y);
        self.send_command(Command::Ramwr);
    }

    /// Draw a vertical line.
    pub fn draw_vertical_line(&mut self, x: u16, y: u16, length: u16, color: u16) {
        self.draw_solid_rect(x, y, 1, length, color);
    }

    /// Draw a horizontal line.
    pub fn draw_horizontal_line(&mut self, x: u16, y: u16, length: u16, color: u16) {
        self.draw_solid_rect(x, y, length, 1, color);
    }

    /// Draw a single pixel.**Not recommended**.
    pub fn pixel(&mut self, x: u16, y: u16, color: u16) {
        self.set_window(x, y, x, y);
        self.send_data(&[(color >> 8) as u8, (color & 0xff) as u8]);
    }

    /// Draw the color buffer into an area.
    ///
    /// The `bitmap` is a color array of `u16`.
    pub fn draw_color_buf(&mut self, bitmap: &[u16], x: u16, y: u16, width: u16, height: u16) {
        assert_eq!(bitmap.len(), width as usize * height as usize);
        self.set_window(x, y, x + width - 1, y + height - 1);
        let chunks = (width * height) / BUFFER_SIZE;
        let rest = (width * height) % BUFFER_SIZE;

        let buf: &mut [u8] = &mut [0u8; BUFFER_SIZE as usize * 2];

        let mut index = 0;
        for _ in 0..chunks {
            for i in 0..BUFFER_SIZE {
                buf[i as usize * 2] = (bitmap[index] >> 8) as u8;
                buf[i as usize * 2 + 1] = (bitmap[index] & 0xff) as u8;
                index += 1;
            }
            self.send_data(buf);
        }
        if rest > 0 {
            for i in 0..rest {
                buf[i as usize * 2] = (bitmap[index] >> 8) as u8;
                buf[i as usize * 2 + 1] = (bitmap[index] & 0xff) as u8;
                index += 1;
            }
            self.send_data(&buf[0..2 * rest as usize]);
        }
    }


    /// Draw the raw color buffer into an area.
    ///
    /// The `buf` is a color array of `u8` which encoded with big-endian.
    pub fn draw_color_buf_raw(&mut self, buffer: &[u8], x: u16, y: u16, width: u16, height: u16) {
        assert_eq!(buffer.len(), width as usize * height as usize * 2);
        self.set_window(x, y, x + width - 1, y + height - 1);
        self.send_data(buffer);
    }

    /// Draw a solid rectangle.
    pub fn draw_solid_rect(&mut self, x: u16, y: u16, width: u16, height: u16, color: u16) {
        self.set_window(x, y, x + width - 1, y + height - 1);
        let pixel: [u8; 2] = [(color >> 8) as u8, (color & 0xff) as u8];
        let chunks = (width * height) / BUFFER_SIZE;
        let rest = (width * height) % BUFFER_SIZE;

        let buf: &mut [u8] = &mut [0u8; BUFFER_SIZE as usize * 2];
        for i in 0..BUFFER_SIZE {
            buf[i as usize * 2] = pixel[0];
            buf[i as usize * 2 + 1] = pixel[1];
        }

        for _ in 0..chunks {
            self.send_data(buf);
        }
        if rest > 0 {
            self.send_data(&buf[0..2 * rest as usize]);
        }
    }

    /// Fill the screen with a color.
    pub fn fill(&mut self, color: u16) {
        self.draw_solid_rect(0, 0, self.width, self.height, color);
    }

    /// Draw a hollow rectangle.
    pub fn draw_hollow_rect(&mut self, x: u16, y: u16, width: u16, height: u16, color: u16) {
        self.draw_horizontal_line(x, y, width, color);
        self.draw_horizontal_line(x, y + height - 1, width, color);
        self.draw_vertical_line(x, y, height, color);
        self.draw_vertical_line(x + width - 1, y, height, color);
    }

    /// Draw a line from (x0, y0) to (x1, y1).
    pub fn line(&mut self, x0: u16, y0: u16, x1: u16, y1: u16) {
        let mut x0 = x0;
        let mut y0 = y0;
        let mut x1 = x1;
        let mut y1 = y1;

        let steep = abs(y1 as i16 - y0 as i16) > abs(x1 as i16 - x0 as i16);
        if steep {
            mem::swap(&mut x0, &mut y0);
            mem::swap(&mut x1, &mut y1);
        }
        if x0 > x1 {
            mem::swap(&mut x0, &mut x1);
            mem::swap(&mut y0, &mut y1);
        }
        let dx: i16 = x1 as i16 - x0 as i16;
        let dy: i16 = y1 as i16 - y0 as i16;
        let mut derror: i16 = (dx / 2) as i16;
        let ystep: i16 = if y0 < y1 { 1 } else { -1 };
        let mut y: i16 = y0 as i16;
        for x in x0..=x1 {
            if steep {
                self.pixel(y as u16, x as u16, 0xffff);
            } else {
                self.pixel(x as u16, y as u16, 0xffff);
            }
            derror -= dy;
            if derror < 0 {
                y += ystep;
                derror += dx;
            }
        }
    }

    /// Set Vertical Scrolling Definition.
    ///
    /// To scroll a 135x240 display these values should be 40, 240, 40.
    /// There are 40 lines above the display that are not shown followed by
    /// 240 lines that are shown followed by 40 more lines that are not shown.
    /// You could write to these areas off display and scroll them into view by
    /// changing the TFA, VSA and BFA values.
    ///
    /// Args:
    ///
    /// tfa (u16): Top Fixed Area
    ///
    /// vsa (u16): Vertical Scrolling Area
    ///
    /// bfa (u16): Bottom Fixed Area
    pub fn vscrdef(&mut self, tfa: u16, vsa: u16, bfa: u16) {
        self.send_command(Command::Vscrdef);
        self.send_data(&tfa.to_be_bytes());
        self.send_data(&vsa.to_be_bytes());
        self.send_data(&bfa.to_be_bytes());
    }

    /// Set Vertical Scroll Start Address of RAM.
    ///
    /// Defines which line in the Frame Memory will be written as the first
    /// line after the last line of the Top Fixed Area on the display.
    ///
    /// Args:
    ///
    /// vssa (u16): Vertical Scrolling Start Address
    pub fn vscsad(&mut self, vssa: u16) {
        self.send_command(Command::Vscsad);
        self.send_data(&vssa.to_be_bytes());
    }

    /// Draw text with a specific font.
    /// `x` and `y` are the top left corner of the text.
    /// Returns the bottom right corner of the text box.
    ///
    /// When meeting a newline character or reach the end of screen, the next line will be drawn.
    ///
    /// When the distance to the bottom of the screen is less than the font height, it will stop drawing.
    pub fn draw_text(&mut self, x: u16, y: u16, text: &str, font: &dyn Font, font_color: u16, background_color: u16) -> (u16, u16) {
        let start_x = x;
        let mut end_x = x;
        let height = font.get_height() as u16;
        let mut x = x;
        let mut y = y;

        let render_buffer = &mut [0u8; BUFFER_SIZE as usize * 2];

        for c in text.chars() {
            if c == '\n' {
                if x > end_x {
                    end_x = x;
                }
                x = start_x;
                y += height;
                if y + height > self.height as u16 {
                    return (end_x, y);
                } else {
                    continue;
                }
            }
            if let Some((buf, w)) = font.get_char(c) {
                if x + w as u16 > self.width {
                    if x > end_x {
                        end_x = x;
                    }
                    x = start_x;
                    y += height;
                    if y + height > self.height as u16 {
                        return (end_x, y);
                    }
                }
                self.set_window(x, y, x + w as u16 - 1, y + height - 1);

                let mut buf_index: usize = 0;
                for i in (0 as usize)..(w as usize * height as usize) {
                    if buf_index == (BUFFER_SIZE * 2) as usize {
                        buf_index = 0;
                        self.send_data(render_buffer);
                    }
                    if buf[i >> 3] & (0x80 >> (i & 7)) != 0 {
                        render_buffer[buf_index] = (font_color >> 8) as u8;
                        render_buffer[buf_index + 1] = (font_color & 0xff) as u8;
                    } else {
                        render_buffer[buf_index] = (background_color >> 8) as u8;
                        render_buffer[buf_index + 1] = (background_color & 0xff) as u8;
                    }
                    buf_index += 2;
                }
                if buf_index != 0 {
                    self.send_data(&render_buffer[0..buf_index]);
                }
                x += w as u16;
            }
        }
        (end_x, y + height as u16)
    }
}
