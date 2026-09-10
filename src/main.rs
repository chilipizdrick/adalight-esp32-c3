#![no_std]
#![no_main]

use core::panic::PanicInfo;

use esp_hal::Async;
use esp_hal::clock::CpuClock;
use esp_hal::spi::master::{Config as SpiConfig, Spi};
use esp_hal::time::Rate;
use esp_hal::timer::timg::TimerGroup;
use esp_hal::usb_serial_jtag::{UsbSerialJtag, UsbSerialJtagRx};

use embassy_executor::Spawner;
use embedded_io_async::Read;
use smart_leds::{RGB8, SmartLedsWrite};
use ws2812_spi::Ws2812;

#[panic_handler]
fn handle_panic(_: &PanicInfo) -> ! {
    esp_hal::system::software_reset()
}

const NUM_LEDS: usize = 102;
const RAW_BUF_SIZE: usize = NUM_LEDS * 3;

esp_bootloader_esp_idf::esp_app_desc!();

#[esp_rtos::main]
async fn main(_spawner: Spawner) -> ! {
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    let sw_interrupt =
        esp_hal::interrupt::software::SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    esp_rtos::start(timg0.timer0, sw_interrupt.software_interrupt0);

    let spi_config = SpiConfig::default().with_frequency(Rate::from_mhz(3));
    let spi = Spi::new(peripherals.SPI2, spi_config)
        .unwrap()
        .with_mosi(peripherals.GPIO4);
    let mut ws2812 = Ws2812::new(spi);

    let usb_serial = UsbSerialJtag::new(peripherals.USB_DEVICE).into_async();
    let (mut rx, _) = usb_serial.split();

    let mut leds = [RGB8::default(); NUM_LEDS];
    let mut raw_bytes = [0u8; RAW_BUF_SIZE];

    loop {
        // Align to ['A', 'd', 'a', high, low, checksum], while checking checksum
        let (high, low) = match sync_header(&mut rx).await {
            Some(counts) => counts,
            None => continue,
        };

        let led_count = (((high as usize) << 8) | (low as usize)) + 1;
        let count_to_read = led_count.min(NUM_LEDS);
        let bytes_to_read = count_to_read * 3;

        // Fill led buffer
        if rx
            .read_exact(&mut raw_bytes[..bytes_to_read])
            .await
            .is_err()
        {
            continue;
        }

        // Drain any trailing bytes if the host configured more LEDs than we support
        if led_count > NUM_LEDS {
            let mut discard = [0u8; 64];
            let mut excess = (led_count - NUM_LEDS) * 3;
            while excess > 0 {
                let chunk = excess.min(discard.len());
                if rx.read_exact(&mut discard[..chunk]).await.is_err() {
                    break;
                }
                excess -= chunk;
            }
        }

        // Map raw RGB bytes into RGB8 structs
        for (i, chunk) in raw_bytes[..bytes_to_read].chunks_exact(3).enumerate() {
            leds[i] = RGB8::new(chunk[0], chunk[1], chunk[2]);
        }

        // Write data to leds
        let _ = ws2812.write(leds);
    }
}

async fn sync_header(rx: &mut UsbSerialJtagRx<'static, Async>) -> Option<(u8, u8)> {
    let mut window = [0u8; 3];

    rx.read_exact(&mut window).await.ok()?;

    while &window != b"Ada" {
        window[0] = window[1];
        window[1] = window[2];
        let mut next_byte = [0u8; 1];
        rx.read_exact(&mut next_byte).await.ok()?;
        window[2] = next_byte[0];
    }

    let mut header_tail = [0u8; 3];
    rx.read_exact(&mut header_tail).await.ok()?;

    let high = header_tail[0];
    let low = header_tail[1];
    let checksum = header_tail[2];

    if checksum == (high ^ low ^ 0x55) {
        Some((high, low))
    } else {
        None
    }
}
