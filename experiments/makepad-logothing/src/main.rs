#![no_std]
#![no_main]

use bsp::hal::{
    clocks::{Clock, init_clocks_and_plls},
    pac,
    sio::Sio,
    watchdog::Watchdog,
    gpio::FunctionUart, 
    uart::{self},
    
};
use cortex_m_rt::entry;
use core::panic::PanicInfo;
use embedded_time::fixed_point::FixedPoint;
use embedded_time::rate::Extensions;
use rp_pico as bsp;
use rp_pico::hal;



use rp2040_st7789::st7789::{Rotation, ST7789Display};
use rp2040_st7789::font::Font;


#[panic_handler]
fn picnic(_info: &PanicInfo) -> ! {
    loop {}
}

fn convrgbto16bit(r: u16, g: u16, b: u16) -> u16{
    // convert 888 to 565 
    ((r & 0b11111000) << 8) | ((g & 0b11111100) << 3) | ((b & 0b11111000) >> 3)    
}
       
fn showlogo(buf: &mut[u8;135*240*2]){
    let b = include_bytes!("portrait_1.tga");
    for x in 0..(135 ) {
        for y in 0..(240 ) 
        {
            let v = convrgbto16bit( b[(x+y*135)*3 + 2 + 18].into(), b[(x+y*135)*3+1+ 18].into(), b[(x+y*135)*3+ 18].into());
            buf[(x + y * 135) * 2] = (v>>8) as u8;
            buf[(x + y * 135) * 2 + 1] = (v&0b11111111) as  u8; 
        }        
    }
}



fn showlog(buf: &mut[u8;135*240*2],loglines: [[char;16];16]){
    let b = include_bytes!("portrait_2_debuglog.tga");
    for x in 0..(135 ) {
        for y in 0..(240 ) 
        {
            let v = convrgbto16bit( b[(x+y*136)*4 + 2 + 18].into(), b[(x+y*136)*4+1+ 18].into(), b[(x+y*136)*4+ 18].into());
            buf[(x + y * 135) * 2] = (v>>8) as u8;
            buf[(x + y * 135) * 2 + 1] = (v&0b11111111) as  u8; 
        }        
    }

    let font = rp2040_st7789::fonts::VGA1_8X16;
    let background_color: u16 = convrgbto16bit(5,35,41);
    let font_color: u16 = convrgbto16bit(250,225,188);
    
    for line in 0..9{
        let mut x: usize = 7;
        let y: usize = 10 + line * 18;
        let height = 16;
        for cx in 0..16{
            if let Some((bbuf, w)) = font.get_char(loglines[line][cx]) {
               
                let mut i = 0;
                for iy in (0 as usize)..( height as usize) {                   
                    for ix in (0 as usize)..(w as usize ) {
                        let buf_index: usize = (ix + x + (iy + y) * 135)*2;
                        if bbuf[i >> 3] & (0x80 >> (i & 7)) != 0 {
                            buf[buf_index] = (font_color >> 8) as u8;
                            buf[buf_index + 1] = (font_color & 0xff) as u8;
                        } else {
                            buf[buf_index] = (background_color >> 8) as u8;
                            buf[buf_index + 1] = (background_color & 0xff) as u8;
                        }
                        i = i + 1;                    
                    }
                }
                x += w as usize;
            }
        }
    }
}


fn add_char(loglines: &mut [[char;16];16], c: char,  line: &mut usize, cur:&mut usize) ->[[char;16];16]{

    let mut new_loglines = *loglines; 
    let mut newline = *line;
    let mut newcur = *cur ;
    if newcur > 15 || c == '\n'
    {
        newline = newline + 1;
        newcur = 0;
        if newline > 10
        {
            // shift lines here.
            for y in 0..10{
                for x in 0..16{
                    new_loglines[y][x] = new_loglines[y+1][x];
                }
            }
            for x in 0..16{
                new_loglines[10][x] = ' ';
            }
            newline = 10;
        }
    }
    if c != '\n'{
      new_loglines[newline][newcur] = c;
      *cur = newcur + 1;
    }
    else{*cur = newcur;}
    *line = newline;
    new_loglines
}


#[entry]
fn main() -> ! {

    let mut loglines: [[char;16];16] = 
    [
        ['M','a','k','e','p','a','d',' ','l','o','g',' ',' ',' ',' ',' '],     
        [' ',' ',' ',' ',' ',' ',' ',' ',' ',' ',' ',' ',' ',' ',' ',' '],     
        [' ',' ',' ',' ',' ',' ',' ',' ',' ',' ',' ',' ',' ',' ',' ',' '],     
        [' ',' ',' ',' ',' ',' ',' ',' ',' ',' ',' ',' ',' ',' ',' ',' '],     
        [' ',' ',' ',' ',' ',' ',' ',' ',' ',' ',' ',' ',' ',' ',' ',' '],     
        [' ',' ',' ',' ',' ',' ',' ',' ',' ',' ',' ',' ',' ',' ',' ',' '],     
        [' ',' ',' ',' ',' ',' ',' ',' ',' ',' ',' ',' ',' ',' ',' ',' '],     
        [' ',' ',' ',' ',' ',' ',' ',' ',' ',' ',' ',' ',' ',' ',' ',' '],     
        [' ',' ',' ',' ',' ',' ',' ',' ',' ',' ',' ',' ',' ',' ',' ',' '],     
        [' ',' ',' ',' ',' ',' ',' ',' ',' ',' ',' ',' ',' ',' ',' ',' '],     
        [' ',' ',' ',' ',' ',' ',' ',' ',' ',' ',' ',' ',' ',' ',' ',' '],     
        [' ',' ',' ',' ',' ',' ',' ',' ',' ',' ',' ',' ',' ',' ',' ',' '],     
        [' ',' ',' ',' ',' ',' ',' ',' ',' ',' ',' ',' ',' ',' ',' ',' '],     
        [' ',' ',' ',' ',' ',' ',' ',' ',' ',' ',' ',' ',' ',' ',' ',' '],     
        [' ',' ',' ',' ',' ',' ',' ',' ',' ',' ',' ',' ',' ',' ',' ',' '],     
        [' ',' ',' ',' ',' ',' ',' ',' ',' ',' ',' ',' ',' ',' ',' ',' ']

    ];
    let mut line:usize = 1;
    let mut cur:usize = 0;

    let mut pac = pac::Peripherals::take().unwrap();
    let core = pac::CorePeripherals::take().unwrap();
    let mut watchdog = Watchdog::new(pac.WATCHDOG);
    let sio = Sio::new(pac.SIO);
    //let mut peripherals = pac::Peripherals::take().unwrap();
    // External high-speed crystal on the pico board is 12Mhz
    let external_xtal_freq_hz = 12_000_000u32;
    let clocks = init_clocks_and_plls(
        external_xtal_freq_hz,
        pac.XOSC,
        pac.CLOCKS,
        pac.PLL_SYS,
        pac.PLL_USB,
        &mut pac.RESETS,
        &mut watchdog,
    )
        .ok()
        .unwrap();

    let mut delay = cortex_m::delay::Delay::new(core.SYST, clocks.system_clock.freq().integer());

    let pins = bsp::Pins::new(
        pac.IO_BANK0,
        pac.PADS_BANK0,
        sio.gpio_bank0,
        &mut pac.RESETS,
    );

    // setup clock and mosi pin
    // TODO: Please replace it with your pin
    pins.gpio3.into_mode::<hal::gpio::FunctionSpi>();
    pins.gpio2.into_mode::<hal::gpio::FunctionSpi>();
    // setup spi
    let spi = hal::Spi::<_, _, 8>::new(pac.SPI0);
    let spi = spi.init(
        &mut pac.RESETS,
        clocks.peripheral_clock.freq(),
        (30u32 << 20u32).Hz(),
        &embedded_hal::spi::MODE_3,
    );

    let mut display = ST7789Display::new(
        pins.gpio0.into_push_pull_output(),
        pins.gpio1.into_push_pull_output(),
        pins.gpio5.into_push_pull_output(),
        pins.gpio4.into_push_pull_output(),
        spi,
        135,
        240,
        Rotation::Portrait,
        &mut delay,
        52,
        40
    );

    let _tx_pin = pins.gpio8.into_mode::<FunctionUart>();
    let _rx_pin = pins.gpio9.into_mode::<FunctionUart>();

    
    let  uart = hal::uart::UartPeripheral::new(pac.UART1, &mut pac.RESETS).enable(
        uart::common_configs::_115200_8_N_1,
        clocks.peripheral_clock.into()
    ).unwrap();

   // uart.enable_rx_interrupt();
    let buf = &mut [0u8; 135 * 240 * 2];
    showlogo(buf);
    display.draw_color_buf_raw(buf, 0, 0, 135, 240);
    delay.delay_ms(500);

    let mut rbuf = [0u8;4096];
    
    loop {
        if let Ok(bytes) = uart.read_raw(&mut rbuf){
            for i in 0..bytes{
                loglines = add_char(&mut loglines, rbuf[i] as char, &mut line, &mut cur);
            }
            showlog(buf, loglines);
            display.draw_color_buf_raw(buf, 0, 0, 135, 240);
        }
        delay.delay_ms(10);
    }
}
