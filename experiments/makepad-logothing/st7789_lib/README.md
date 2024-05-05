# rp2040-st7789 driver

This is a rust driver for ST7789 on pico.It mainly based on [rp-rs/rp-hal](https://github.com/rp-rs/rp-hal).

The driver support for:
- 320x240, 240x240 and 135x240 pixel displays
- Display rotation
- Hardware based scrolling
- Drawing text using 8 and 16 bit wide bitmap fonts with heights that are multiples of 8. Included are 12 bitmap fonts derived from classic pc BIOS text mode fonts.
- Drawing text using converted TrueType fonts.
- Drawing converted bitmaps

## Get Started

Add this to `Cargo.toml`
```toml
rp2040-st7789 = { version = "0.1.0", git = "https://github.com/ri-char/`rp2040-st7789`"}
```

## Font

The project contains two fonts and 6 different sizes each type.

You can add your own font by implementing `trait Font`.

## Example

The example is at [./example](./example).

## Reference
1. [russhughes/st7789py_mpy](https://github.com/russhughes/st7789py_mpy)
