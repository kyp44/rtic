//! [`Monotonic`](rtic_time::Monotonic) implementation for the ATSAMD Real Time Clocks (RTC).
//!
//! # Example
//!
//! ```
//! use rtic_monotonics::atsamd::prelude::*;
//! atsamd_rtc_monotonic!(Mono);
//!
//! fn init() {
//!     # // This is normally provided by the selected PAC
//!     # let rtc = unsafe { core::mem::transmute(()) };
//!     // Start the monotonic
//!     Mono::start(rtc);
//! }
//!
//! async fn usage() {
//!     loop {
//!          // Use the monotonic
//!          let timestamp = Mono::now();
//!          Mono::delay(100.millis()).await;
//!     }
//! }
//! ```

/// Common definitions and traits for using the ATSAMD RTC monotonics
pub mod prelude {
    pub use crate::atsamd_rtc_monotonic;

    pub use crate::Monotonic;
    pub use fugit::{self, ExtU64, ExtU64Ceil};
}

#[cfg(feature = "atsamd51j")]
#[doc(hidden)]
pub use atsamd51j::{self as pac, RTC};

use portable_atomic::{AtomicU32, Ordering};
use rtic_time::{
    half_period_counter::calculate_now,
    timer_queue::{TimerQueue, TimerQueueBackend},
};

#[doc(hidden)]
#[macro_export]
macro_rules! __internal_create_atsamd_rtc_interrupt {
    ($mono_backend:ident, $rtc:ident) => {
        #[no_mangle]
        #[allow(non_snake_case)]
        unsafe extern "C" fn $rtc() {
            use $crate::TimerQueueBackend;
            $crate::atsamd::rtc::$mono_backend::timer_queue().on_monotonic_interrupt();
        }
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __internal_create_atsamd_rtc_struct {
    ($name:ident, $mono_backend:ident, $timer:ident) => {
        /// A `Monotonic` based on the ATSAMD RTC peripheral.
        pub struct $name;

        impl $name {
            /// Starts the `Monotonic`.
            ///
            /// This method must be called only once.
            pub fn start(rtc: $crate::atsamd::rtc::$timer) {
                $crate::__internal_create_atsamd_rtc_interrupt!($mono_backend, $timer);

                $crate::atsamd::rtc::$mono_backend::_start(rtc);
            }
        }

        impl $crate::TimerQueueBasedMonotonic for $name {
            type Backend = $crate::atsamd::rtc::$mono_backend;
            type Instant = $crate::fugit::Instant<
                <Self::Backend as $crate::TimerQueueBackend>::Ticks,
                1,
                32_768,
            >;
            type Duration = $crate::fugit::Duration<
                <Self::Backend as $crate::TimerQueueBackend>::Ticks,
                1,
                32_768,
            >;
        }

        $crate::rtic_time::impl_embedded_hal_delay_fugit!($name);
        $crate::rtic_time::impl_embedded_hal_async_delay_fugit!($name);
    };
}

/// Create an RTC based monotonic and register the RTC interrupt for it.
///
/// See [`crate::atsamd::rtc`] for more details.
#[macro_export]
macro_rules! atsamd_rtc_monotonic {
    ($name:ident) => {
        $crate::__internal_create_atsamd_rtc_struct!($name, RtcBackend, RTC);
    };
}

struct TimerValueU32(u32);
impl rtic_time::half_period_counter::TimerValue for TimerValueU32 {
    const BITS: u32 = 32;
}
impl From<TimerValueU32> for u64 {
    fn from(value: TimerValueU32) -> Self {
        Self::from(value.0)
    }
}

macro_rules! make_rtc {
    ($backend_name:ident, $rtc:ident, $overflow:ident, $tq:ident$(, doc: ($($doc:tt)*))?) => {
        /// RTC based [`TimerQueueBackend`].
        $(
            #[cfg_attr(docsrs, doc(cfg($($doc)*)))]
        )?
        pub struct $backend_name;

        static $overflow: AtomicU32 = AtomicU32::new(0);
        static $tq: TimerQueue<$backend_name> = TimerQueue::new();

        impl $backend_name {
            /// Starts the timer.
            ///
            /// **Do not use this function directly.**
            ///
            /// Use the prelude macros instead.
            pub fn _start(rtc: $rtc) {
                unsafe { rtc.mode0().write(|w| w.bits(0)) };

                // Disable interrupts, as preparation
                rtc.intenclr.write(|w| w
                    .compare0().clear()
                    .compare1().clear()
                    .ovrflw().clear()
                );

                // Configure compare registers
                rtc.cc[0].write(|w| unsafe { w.bits(0) }); // Dynamic wakeup
                rtc.cc[1].write(|w| unsafe { w.bits(0x80_0000) }); // Half-period

                // Timing critical, make sure we don't get interrupted
                critical_section::with(|_|{
                    // Reset the timer
                    rtc.tasks_clear.write(|w| unsafe { w.bits(1) });
                    rtc.tasks_start.write(|w| unsafe { w.bits(1) });

                    // Clear pending events.
                    // Should be close enough to the timer reset that we don't miss any events.
                    rtc.events_ovrflw.write(|w| w);
                    rtc.events_compare[0].write(|w| w);
                    rtc.events_compare[1].write(|w| w);

                    // Make sure overflow counter is synced with the timer value
                    $overflow.store(0, Ordering::SeqCst);

                    // Initialized the timer queue
                    $tq.initialize(Self {});

                    // Enable interrupts.
                    // Should be close enough to the timer reset that we don't miss any events.
                    rtc.intenset.write(|w| w
                        .compare0().set()
                        .compare1().set()
                        .ovrflw().set()
                    );
                    rtc.evtenset.write(|w| w
                        .compare0().set()
                        .compare1().set()
                        .ovrflw().set()
                    );
                });

                // SAFETY: We take full ownership of the peripheral and interrupt vector,
                // plus we are not using any external shared resources so we won't impact
                // basepri/source masking based critical sections.
                unsafe {
                    crate::set_monotonic_prio(pac::NVIC_PRIO_BITS, pac::Interrupt::$rtc);
                    pac::NVIC::unmask(pac::Interrupt::$rtc);
                }
            }
        }

        impl TimerQueueBackend for $backend_name {
            type Ticks = u64;

            fn now() -> Self::Ticks {
                let rtc = unsafe { &*$rtc::PTR };
                calculate_now(
                    || $overflow.load(Ordering::Relaxed),
                    || TimerValueU24(rtc.counter.read().bits())
                )
            }

            fn on_interrupt() {
                let rtc = unsafe { &*$rtc::PTR };
                if rtc.events_ovrflw.read().bits() == 1 {
                    rtc.events_ovrflw.write(|w| unsafe { w.bits(0) });
                    let prev = $overflow.fetch_add(1, Ordering::Relaxed);
                    assert!(prev % 2 == 1, "Monotonic must have skipped an interrupt!");
                }
                if rtc.events_compare[1].read().bits() == 1 {
                    rtc.events_compare[1].write(|w| unsafe { w.bits(0) });
                    let prev = $overflow.fetch_add(1, Ordering::Relaxed);
                    assert!(prev % 2 == 0, "Monotonic must have skipped an interrupt!");
                }
            }

            fn set_compare(mut instant: Self::Ticks) {
                let rtc = unsafe { &*$rtc::PTR };

                const MAX: u64 = 0xff_ffff;

                // Disable interrupts because this section is timing critical.
                // We rely on the fact that this entire section runs within one
                // RTC clock tick. (which it will do easily if it doesn't get
                // interrupted)
                critical_section::with(|_|{
                    let now = Self::now();
                    // wrapping_sub deals with the u64 overflow corner case
                    let diff = instant.wrapping_sub(now);
                    let val = if diff <= MAX {
                        // Now we know `instant` whill happen within one `MAX` time duration.

                        // Errata: Timer interrupts don't fire if they are scheduled less than
                        // two ticks in the future. Make it three, because the timer could
                        // tick right now.
                        if diff < 3 {
                            instant = now.wrapping_add(3);
                        }

                        (instant & MAX) as u32
                    } else {
                        0
                    };

                    unsafe { rtc.cc[0].write(|w| w.bits(val)) };
                });
            }

            fn clear_compare_flag() {
                let rtc = unsafe { &*$rtc::PTR };
                unsafe { rtc.events_compare[0].write(|w| w.bits(0)) };
            }

            fn pend_interrupt() {
                pac::NVIC::pend(pac::Interrupt::$rtc);
            }

            fn timer_queue() -> &'static TimerQueue<Self> {
                &$tq
            }
        }
    };
}

make_rtc!(RtcBackend, RTC, RTC_OVERFLOWS, RTC_TQ);
