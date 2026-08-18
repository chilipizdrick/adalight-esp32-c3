#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]

use core::panic::PanicInfo;

use esp_hal::clock::CpuClock;
use esp_hal::spi::master::{Config as SpiConfig, Spi};
use esp_hal::time::Rate;
use esp_hal::usb_serial_jtag::UsbSerialJtag;
use esp_hal::{Blocking, main};

use smart_leds::{RGB8, SmartLedsWrite};
use ws2812_spi::Ws2812;

#[panic_handler]
fn handle_panic(_: &PanicInfo) -> ! {
    loop {}
}

const NUM_LEDS: usize = 102;

esp_bootloader_esp_idf::esp_app_desc!();

#[main]
fn main() -> ! {
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    let spi_config = SpiConfig::default().with_frequency(Rate::from_mhz(3));
    let spi = Spi::new(peripherals.SPI2, spi_config)
        .unwrap()
        .with_mosi(peripherals.GPIO4);
    let mut ws2812 = Ws2812::new(spi);

    let mut uart = UsbSerialJtag::new(peripherals.USB_DEVICE);

    let mut leds = [RGB8::default(); NUM_LEDS];

    loop {
        if !read_magic_word(&mut uart) {
            continue;
        }

        let high = read_byte(&mut uart);
        let low = read_byte(&mut uart);
        let checksum = read_byte(&mut uart);
        if checksum != (high ^ low ^ 0x55) {
            continue;
        }

        let led_count = (((high as usize) << 8) | (low as usize)) + 1;
        let count_to_read = led_count.min(NUM_LEDS);
        for i in 0..count_to_read {
            let r = read_byte(&mut uart);
            let g = read_byte(&mut uart);
            let b = read_byte(&mut uart);
            leds[i] = RGB8::new(r, g, b);
        }

        let _ = ws2812.write(leds);
    }
}

fn read_magic_word(uart: &mut UsbSerialJtag<'static, Blocking>) -> bool {
    read_byte(uart) == b'A' && read_byte(uart) == b'd' && read_byte(uart) == b'a'
}

fn read_byte(uart: &mut UsbSerialJtag<'static, Blocking>) -> u8 {
    loop {
        match uart.read_byte() {
            Ok(byte) => return byte,
            Err(_) => continue,
        }
    }
}
