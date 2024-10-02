#![deny(unsafe_code)]
#![deny(warnings)]
#![no_main]
#![no_std]

// TODO: For now use SysTick to make sure this works, until the RTC driver is complete
//use rtic_monotonics::atsamd::rtc::prelude::*;
use rtic_monotonics::systick::prelude::*;

systick_monotonic!(Mono, 1000);

#[rtic::app(device = pygamer::pac, dispatchers = [TC0])]
mod app {
    use super::*;
    use pygamer::{
        hal::{clock::GenericClockController, prelude::*},
        Pins, RedLed,
    };

    #[shared]
    struct Shared {}

    #[local]
    struct Local {
        red_led: RedLed,
    }

    #[init]
    fn init(mut cx: init::Context) -> (Shared, Local) {
        // Set up the clocks
        // TODO: May not need this once the RTC driver is used.
        let _ = GenericClockController::with_internal_32kosc(
            cx.device.GCLK,
            &mut cx.device.MCLK,
            &mut cx.device.OSC32KCTRL,
            &mut cx.device.OSCCTRL,
            &mut cx.device.NVMCTRL,
        );

        // Start the SysTick
        Mono::start(cx.core.SYST, 120_000_000);

        let pins = Pins::new(cx.device.PORT);
        let red_led = pins.d13.into();

        // Schedule the blinking task
        blink::spawn().unwrap();

        (Shared {}, Local { red_led })
    }

    #[task(local = [red_led])]
    async fn blink(cx: blink::Context) {
        let mut blink_on = false;

        loop {
            Mono::delay(500.millis()).await;

            blink_on = !blink_on;
            if blink_on {
                cx.local.red_led.set_high().unwrap();
            } else {
                cx.local.red_led.set_low().unwrap();
            }
        }
    }
}
