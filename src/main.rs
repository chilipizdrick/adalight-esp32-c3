#![no_std]
#![no_main]

use core::panic::PanicInfo;

use embassy_time::{Duration, with_timeout};
use embedded_hal::spi::SpiBus;
use esp_hal::Async;
use esp_hal::clock::CpuClock;
use esp_hal::interrupt::software::SoftwareInterruptControl;
use esp_hal::spi::master::{Config as SpiConfig, Spi};
use esp_hal::time::Rate;
use esp_hal::timer::timg::TimerGroup;
use esp_hal::usb_serial_jtag::{UsbSerialJtag, UsbSerialJtagRx};

use embassy_executor::Spawner;
use embedded_io_async::Read;
use smart_leds::colors::BLACK;
use smart_leds::{RGB8, SmartLedsWrite};
use ws2812_spi::Ws2812;

const NUM_LEDS: usize = 102;
const RX_TIMEOUT: Duration = Duration::from_secs(3);

#[panic_handler]
fn handle_panic(_: &PanicInfo) -> ! {
    esp_hal::system::software_reset()
}

esp_bootloader_esp_idf::esp_app_desc!();

#[esp_rtos::main]
async fn main(_spawner: Spawner) -> ! {
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    let sw_interrupt = SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    esp_rtos::start(timg0.timer0, sw_interrupt.software_interrupt0);

    let spi_config = SpiConfig::default().with_frequency(Rate::from_mhz(3));
    let spi = Spi::new(peripherals.SPI2, spi_config)
        .unwrap()
        .with_mosi(peripherals.GPIO4);
    let driver = Ws2812::new(spi);
    let mut leds = LedsAdapter::new(driver);

    let usb_serial = UsbSerialJtag::new(peripherals.USB_DEVICE).into_async();
    let (mut rx, _) = usb_serial.split();

    loop {
        // Align to ['A', 'd', 'a', high, low, checksum], while checking checksum
        let (high, low) = match with_timeout(RX_TIMEOUT, sync_header(&mut rx)).await {
            Ok(Some(counts)) => counts,
            Ok(None) => continue,
            Err(_) => {
                let _ = leds.write_black_if_on();
                continue;
            }
        };

        let led_count = (((high as usize) << 8) | (low as usize)) + 1;
        let count_to_read = led_count.min(NUM_LEDS);
        let bytes_to_read = count_to_read * 3;

        // Fill led buffer
        let raw_pixel_bytes = leds.led_buffer();
        let payload_res = with_timeout(
            RX_TIMEOUT,
            rx.read_exact(&mut raw_pixel_bytes[..bytes_to_read]),
        )
        .await;
        if payload_res.is_err() {
            let _ = leds.write_black_if_on();
            continue;
        }

        // Drain any trailing bytes if the host configured more LEDs than we support
        if led_count > NUM_LEDS {
            let mut discard = [0u8; 64];
            let mut excess = (led_count - NUM_LEDS) * 3;
            while excess > 0 {
                let chunk = excess.min(discard.len());
                let drain_res =
                    with_timeout(RX_TIMEOUT, rx.read_exact(&mut discard[..chunk])).await;
                if drain_res.is_err() {
                    break;
                }
                excess -= chunk;
            }
        }

        // Write data to leds
        let _ = leds.write();
    }
}

struct LedsAdapter<SPI> {
    driver: Ws2812<SPI>,
    pixels: [RGB8; NUM_LEDS],
    is_lit: bool,
}

impl<SPI, E> LedsAdapter<SPI>
where
    SPI: SpiBus<u8, Error = E>,
{
    fn new(driver: Ws2812<SPI>) -> Self {
        Self {
            driver,
            pixels: [RGB8::default(); NUM_LEDS],
            is_lit: false,
        }
    }

    fn write_black(&mut self) -> Result<(), E> {
        self.pixels.fill(BLACK);
        self.driver.write(self.pixels)?;
        self.is_lit = false;

        Ok(())
    }

    fn write_black_if_on(&mut self) -> Result<(), E> {
        if self.is_lit {
            self.write_black()?;
        }
        Ok(())
    }

    // Access to internal buffer as inline buffer of RGB u8 values
    fn led_buffer(&mut self) -> &mut [u8] {
        let ptr = self.pixels.as_mut_ptr() as *mut u8;
        // SAFETY: &mut [u8] slice with three times the length has the same memory layout as &mut [RGB8], since RGB8 is repr(C)
        unsafe { core::slice::from_raw_parts_mut(ptr, NUM_LEDS * 3) }
    }

    fn write(&mut self) -> Result<(), E> {
        self.driver.write(self.pixels)?;
        self.is_lit = true;

        Ok(())
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
