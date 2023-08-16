#![no_std]
#![no_main]
#![feature(type_alias_impl_trait)]

use clocklib::ClockDisplay;
use defmt::*;
use embassy_executor::Spawner;
use embassy_futures::select::select;
use embassy_futures::select::Either;
use embassy_rp::gpio;
use embassy_rp::gpio::AnyPin;
use embassy_rp::gpio::Pin;
use embassy_rp::i2c::Blocking;
use embassy_rp::i2c::I2c;
use embassy_rp::i2c::{self, Config};
use embassy_rp::peripherals::I2C0;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_sync::blocking_mutex::raw::ThreadModeRawMutex;
use embassy_sync::channel::Channel;
use embassy_sync::mutex::Mutex;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Ticker, Timer};
use gpio::{Input, Level, Output, Pull};
use is31fl3731_driver::IS31FL3731;
use pcf8563::*;
use shared_bus::I2cProxy;
use shared_bus::NullMutex;
use static_cell::make_static;
use {defmt_rtt as _, panic_probe as _};

type StaticClockDisplay = ClockDisplay<I2cProxy<'static, NullMutex<I2c<'static, I2C0, Blocking>>>>;
type StaticRtc =
    Mutex<NoopRawMutex, PCF8563<I2cProxy<'static, NullMutex<I2c<'static, I2C0, Blocking>>>>>;

#[embassy_executor::task]
async fn screen_update(mut clock: StaticClockDisplay) {
    let mut blink: Option<BlinkData> = None;

    loop {
        let time = *CURRENT_TIME.lock().await;
        let brightness_level = *CURRENT_BRIGHTNESS.lock().await;
        let brightness = BRIGHTNESS_MAP[brightness_level];

        info!(
            "Screen refresh: {}:{}:{} {})",
            time.hours, time.minutes, time.seconds, blink
        );

        let digits: [usize; 4] = [
            (time.hours / 10).into(),
            (time.hours % 10).into(),
            (time.minutes / 10).into(),
            (time.minutes % 10).into(),
        ];

        for (i, digit) in digits.iter().enumerate() {
            let mut color = brightness;

            if let Some(blink) = &blink {
                match (blink.frame, blink.position, i) {
                    (0, 0, 2..=3) => {
                        color = 0x02;
                    }
                    (0, 1, 0..=1) => {
                        color = 0x02;
                    }
                    _ => {}
                }
            }

            clock.draw_symbol(i as u8, *digit, color).unwrap();
        }

        let refresh_signal = select(
            Timer::after(Duration::from_millis(20 * 1000)),
            SCREEN_REFRESH_SIGNAL.wait(),
        )
        .await;

        match refresh_signal {
            Either::Second(ScreenRefresh::Blink(blink_data)) => {
                blink.replace(blink_data);
            }
            Either::Second(ScreenRefresh::TimeChanged) => {}
            Either::First(_) | Either::Second(ScreenRefresh::Normal) => blink = None,
        }
    }
}

#[embassy_executor::task]
async fn sync_time(rtc: &'static StaticRtc) {
    loop {
        let synced_time = {
            let mut rtc = rtc.lock().await;
            rtc.get_datetime().unwrap()
        };

        {
            let mut time = CURRENT_TIME.lock().await;
            *time = synced_time;
        }

        Timer::after(Duration::from_secs(60 * 10)).await;
    }
}

#[embassy_executor::task]
async fn led_test(mut clock: StaticClockDisplay) {
    let mut cnt: u8 = 0;

    loop {
        for driver in clock.drivers.iter_mut().flatten() {
            driver.set_color_byte(cnt, 0x36).unwrap();
            if cnt > 0 {
                driver.set_color_byte(cnt - 1, 0x00).unwrap();
            }
        }

        Timer::after(Duration::from_millis(200)).await;
        cnt = (cnt + 1) % 144;
    }
}

#[embassy_executor::task]
async fn led_numbers_test(mut clock: StaticClockDisplay) {
    let mut cnt: usize = 0;

    loop {
        for i in 0..=4 {
            clock.draw_symbol(i, cnt, 0x70).unwrap();
        }

        Timer::after(Duration::from_millis(200)).await;
        cnt = (cnt + 1) % 10;
    }
}

async fn wait_for_low_debounced(button: &mut Input<'_, AnyPin>) {
    loop {
        button.wait_for_low().await;
        let b = select(
            button.wait_for_high(),
            Timer::after(Duration::from_millis(50)),
        )
        .await;

        if let Either::First(_) = b {
            // info!("Bounce");
            continue;
        } else {
            break;
        }
    }
}

#[derive(Format)]
enum Event {
    SetButton(ButtonPress),
    AdjustButton(ButtonPress),
}

#[derive(Format)]
enum ButtonPress {
    Short,
    Long,
}

async fn on_event(event: Event, rtc: &'static StaticRtc) {
    let state = { CURRENT_STATE.lock().await.clone() };

    match (event, state) {
        // Enter time setting mode
        (Event::SetButton(ButtonPress::Long), State::Idle) => to_state(State::SettingTime(0)).await,

        // Exit time setting mode
        (Event::SetButton(ButtonPress::Long), State::SettingTime(_)) => to_state(State::Idle).await,

        // Move to next position
        (Event::SetButton(ButtonPress::Short), State::SettingTime(digit)) => {
            let next_digit = (digit + 1) % 3; // hours, minutes, done
            if next_digit == 2 {
                SCREEN_REFRESH_SIGNAL.signal(ScreenRefresh::Normal);
                to_state(State::Idle).await;
            } else {
                to_state(State::SettingTime(next_digit)).await;
            }
        }

        // Advance hours or minutes
        (Event::AdjustButton(ButtonPress::Short), State::SettingTime(digit)) => {
            advance_time(digit, rtc).await;
        }

        // Adjust brightness
        (Event::AdjustButton(ButtonPress::Short), State::Idle) => adjust_brightness().await,

        (_, _) => {}
    }
}

async fn advance_time(position: u8, rtc: &'static StaticRtc) {
    let mut rtc = rtc.lock().await;
    let mut current = rtc.get_datetime().unwrap();
    let defaut = default_datetime();

    current.month = defaut.month;
    current.weekday = defaut.weekday;
    current.year = defaut.year;

    info!("Advancing time");

    if position == 0 {
        let minutes = (current.minutes + 1) % 60;
        current.minutes = minutes;
        current.seconds = 0;
    } else if position == 1 {
        let hours = (current.hours + 1) % 24;
        current.hours = hours;
    }

    rtc.set_datetime(&current).unwrap();
    *CURRENT_TIME.lock().await = current;
    SCREEN_REFRESH_SIGNAL.signal(ScreenRefresh::TimeChanged);
}

async fn adjust_brightness() {
    let mut brightness = CURRENT_BRIGHTNESS.lock().await;
    *brightness = (*brightness + 1) % 6;
    SCREEN_REFRESH_SIGNAL.signal(ScreenRefresh::Normal);
}

async fn to_state(new_state: State) {
    let state = { CURRENT_STATE.lock().await.clone() };
    info!("State change: {} -> {}", state, new_state);

    let mut state = CURRENT_STATE.lock().await;
    *state = new_state;
}

#[embassy_executor::task]
async fn blink_task() {
    let mut blink_frame: u8 = 0;

    loop {
        let state = { CURRENT_STATE.lock().await.clone() };
        if let State::SettingTime(digit) = state {
            SCREEN_REFRESH_SIGNAL.signal(ScreenRefresh::Blink(BlinkData {
                position: digit as usize,
                frame: blink_frame,
            }));
            blink_frame = (blink_frame + 1) % 2;
        } else {
            blink_frame = 0;
        }
        Timer::after(Duration::from_millis(500)).await;
    }
}

#[embassy_executor::task]
async fn button1_task(button_pin1: AnyPin) {
    let mut button = Input::new(button_pin1, Pull::Up);

    loop {
        wait_for_low_debounced(&mut button).await;
        // info!("Button1 pressed!");

        let a = select(button.wait_for_high(), Timer::after(Duration::from_secs(3))).await;

        match a {
            Either::First(_) => {
                send_event(Event::SetButton(ButtonPress::Short)).await;
            }
            Either::Second(_) => {
                send_event(Event::SetButton(ButtonPress::Long)).await;
                button.wait_for_high().await;
            }
        }
    }
}

#[embassy_executor::task]
async fn button2_task(button_pin: AnyPin) {
    let mut button = Input::new(button_pin, Pull::Up);

    loop {
        wait_for_low_debounced(&mut button).await;
        // info!("Button2 pressed!");

        let a = select(button.wait_for_high(), Timer::after(Duration::from_secs(3))).await;

        match a {
            Either::First(_) => {
                send_event(Event::AdjustButton(ButtonPress::Short)).await;
            }
            Either::Second(_) => {
                send_event(Event::AdjustButton(ButtonPress::Long)).await;
                button.wait_for_high().await;
            }
        }
    }
}

#[embassy_executor::task]
async fn process_events(rtc: &'static StaticRtc) {
    loop {
        let event = EVENT_CHANNEL.recv().await;
        info!("Event: {}", event);
        on_event(event, rtc).await;
    }
}

#[embassy_executor::task]
async fn run_time() {
    let mut ticker = Ticker::every(Duration::from_secs(1));
    loop {
        {
            let mut time = CURRENT_TIME.lock().await;
            time.seconds += 1;

            if time.seconds >= 60 {
                time.seconds = 0;
                time.minutes += 1;
            }

            if time.minutes >= 60 {
                time.minutes = 0;
                time.hours += 1;
            }

            if time.hours >= 24 {
                time.hours = 0;
                time.day += 1;
            }

            // ignoring calendar for now
        }

        ticker.next().await;
    }
}

async fn send_event(event: Event) {
    EVENT_CHANNEL.send(event).await;
}

#[derive(Clone, Format)]
enum State {
    Idle,
    SettingTime(u8), //position: minutes, hours
}

#[derive(Clone, Format)]
enum ScreenRefresh {
    TimeChanged,
    Blink(BlinkData),
    Normal,
}

#[derive(Clone, Format)]
struct BlinkData {
    position: usize,
    frame: u8,
}

const fn default_datetime() -> DateTime {
    DateTime {
        year: 0,
        month: 1,
        weekday: 0,
        day: 1,
        hours: 0,
        minutes: 0,
        seconds: 0,
    }
}

static CURRENT_STATE: Mutex<CriticalSectionRawMutex, State> = Mutex::new(State::Idle);
static CURRENT_TIME: Mutex<ThreadModeRawMutex, DateTime> = Mutex::new(default_datetime());

const MAX_BRIGHTNESS_LEVEL: usize = 6;
const BRIGHTNESS_MAP: [u8; MAX_BRIGHTNESS_LEVEL] = [0x05, 0x10, 0x20, 0x40, 0x60, 0x90];
static CURRENT_BRIGHTNESS: Mutex<ThreadModeRawMutex, usize> = Mutex::new(3); // 0 - 6

static EVENT_CHANNEL: Channel<ThreadModeRawMutex, Event, 10> = Channel::new();
static SCREEN_REFRESH_SIGNAL: Signal<ThreadModeRawMutex, ScreenRefresh> = Signal::new();

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    info!("Start");

    let p = embassy_rp::init(Default::default());

    let mut config = Config::default();
    config.frequency = 400_000;
    let sda = p.PIN_28;
    let scl = p.PIN_29;
    let i2c = i2c::I2c::new_blocking(p.I2C0, scl, sda, config);
    let shared_i2c = make_static!(shared_bus::BusManagerSimple::new(i2c));

    let leds1 = IS31FL3731::new(shared_i2c.acquire_i2c(), 0x76);
    let leds2 = IS31FL3731::new(shared_i2c.acquire_i2c(), 0x74);
    let mut clock = ClockDisplay::new([Some(leds1), Some(leds2), None]);
    clock.setup().unwrap();

    let mut rtc = PCF8563::new(shared_i2c.acquire_i2c());
    rtc.rtc_init().unwrap();
    rtc.control_clkout(Control::Off).unwrap();

    let rtc = make_static!(Mutex::new(rtc));

    unwrap!(spawner.spawn(sync_time(rtc)));
    unwrap!(spawner.spawn(run_time()));
    Timer::after(Duration::from_millis(10)).await;
    unwrap!(spawner.spawn(screen_update(clock)));

    unwrap!(spawner.spawn(button1_task(p.PIN_2.degrade())));
    unwrap!(spawner.spawn(button2_task(p.PIN_3.degrade())));
    unwrap!(spawner.spawn(blink_task()));
    unwrap!(spawner.spawn(process_events(rtc)));
}
