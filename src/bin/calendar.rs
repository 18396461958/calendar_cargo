//! STM32F103 Blue Pill RTC日历系统（带OLED显示）
//! =============================================================================================
//! 
//! 日期        作者          说明
//! 2025/7/20   YHY           初始版本
//! 修改者
//! lin
//! 
//!==============================================================================================
//! 
//! 本固件实现的功能：
//! - 使用SSD1306 OLED显示屏（128x64）通过I2C1通信
//! - 旋转编码器用于时间调整
//! - 按钮用于模式切换
//!
//! 硬件连接：
//!   OLED显示屏 -> Blue Pill开发板
//!      GND  -> GND
//!      VCC  -> 5V
//!      SDA  -> PB7
//!      SCL  -> PB6
//!
//!   旋转编码器：
//!      CLK  -> PA8 (TIM1_CH1)
//!      DT   -> PA9 (TIM1_CH2)
//!      SW   -> PB15 (使用上拉电阻)
//!
//! 主要功能：
//! 1. 实时时钟显示日期和星期
//! 2. 带视觉光标的时间调整界面
//! 3. 旋转编码器修改数值
//! 4. 按钮选择调整字段
//! 5. 板载LED心跳指示灯

#![no_std]          // 不使用标准库
#![no_main]         // 不使用标准main函数

use chrono::{Datelike, NaiveDate, NaiveDateTime, Timelike}; // 添加 Timelike trait
use core::fmt::Write;
use defmt_rtt as _;
use embassy_executor::Spawner;
use embassy_stm32::{
    bind_interrupts,
    gpio::{Output, Speed, Level, Pull},
    i2c::{self, EventInterruptHandler, ErrorInterruptHandler, Master},
    peripherals,
    time::Hertz,
    exti::ExtiInput,
};
use embedded_graphics::{
    mono_font::{ascii::FONT_8X13, ascii::FONT_10X20, MonoTextStyle},
    pixelcolor::BinaryColor,
    primitives::{Line, PrimitiveStyle},
    prelude::*,
    text::{Baseline, Text},
};
use embassy_sync::{
    blocking_mutex::raw::ThreadModeRawMutex,
    channel::{Channel, Receiver, Sender},
};
use embassy_time::{Ticker, Timer, Instant};
use panic_probe as _;
use ssd1306::{prelude::*, I2CDisplayInterface, Ssd1306};

// 通道
static RTC_CHANNEL: Channel<ThreadModeRawMutex, NaiveDateTime, 1> = Channel::new();
static KEY_CHANNEL: Channel<ThreadModeRawMutex, u8, 1> = Channel::new();

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    let p = embassy_stm32::init(Default::default());

    bind_interrupts!(struct Irqs {
        I2C1_EV => EventInterruptHandler<peripherals::I2C1>;
        I2C1_ER => ErrorInterruptHandler<peripherals::I2C1>;
    });
    
    let mut config = i2c::Config::default();
    config.frequency = Hertz(400_000);
    
    let i2c = i2c::I2c::new(
        p.I2C1,
        p.PB6,
        p.PB7,
        Irqs,
        p.DMA1_CH6,
        p.DMA1_CH7,
        config,
    );

    let key_exti = ExtiInput::new(p.PB15, p.EXTI15, Pull::Up);

    _spawner.spawn(oled_display(
        i2c, 
        RTC_CHANNEL.receiver(), 
        KEY_CHANNEL.receiver(),
        embassy_time::Duration::from_millis(200)
    )).unwrap();

    _spawner.spawn(rtc_update(
        RTC_CHANNEL.sender(), 
        embassy_time::Duration::from_millis(20)
    )).unwrap();

    _spawner.spawn(key_update(
        key_exti, 
        KEY_CHANNEL.sender(),
        embassy_time::Duration::from_millis(15)
    )).unwrap();

    let mut led = Output::new(p.PC13, Level::High, Speed::Low);
    let mut ticker = Ticker::every(embassy_time::Duration::from_millis(500));
    
    loop {
        led.set_low();
        ticker.next().await;
        led.set_high();
        ticker.next().await;
    }
}

#[embassy_executor::task]
async fn oled_display(
    i2c: i2c::I2c<'static, embassy_stm32::mode::Async, Master>,
    rtc_channel: Receiver<'static, ThreadModeRawMutex, NaiveDateTime, 1>, 
    key_channel: Receiver<'static, ThreadModeRawMutex, u8, 1>,
    delay: embassy_time::Duration
) {
    let mut ticker = Ticker::every(delay);
    ticker.next().await;

    let interface = I2CDisplayInterface::new(i2c);
    let mut display = Ssd1306::new(
        interface, 
        DisplaySize128x64,
        DisplayRotation::Rotate0
    ).into_buffered_graphics_mode();
    
    // 增强错误处理
    for _ in 0..3 {
        if display.init().is_ok() {
            break;
        }
        Timer::after(embassy_time::Duration::from_millis(100)).await;
    }

    let year_month_day_style = MonoTextStyle::new(
        &FONT_8X13,
        BinaryColor::On,
    );

    let hour_minute_second_style = MonoTextStyle::new(
        &FONT_10X20,
        BinaryColor::On,
    );

    let weekday_style = MonoTextStyle::new(
        &FONT_8X13,
        BinaryColor::On,
    );

    // 星期字符串
    const WEEKDAYS: [&str; 7] = ["星期一", "星期二", "星期三", "星期四", "星期五", "星期六", "星期日"];
    
    let mut cursor_visible = false;
    let mut last_blink_time = Instant::now();
    const BLINK_INTERVAL: embassy_time::Duration = embassy_time::Duration::from_millis(500);

    const CURSOR_POSITIONS: [(Point, Point); 6] = [
        (Point::new(24, 18), Point::new(24 + 4 * 8, 18)),
        (Point::new(24 + 5 * 8, 18), Point::new(24 + 7 * 8, 18)),
        (Point::new(24 + 8 * 8, 18), Point::new(24 + 10 * 8, 18)),
        (Point::new(24, 40), Point::new(24 + 2 * 10, 40)),
        (Point::new(24 + 3 * 10, 40), Point::new(24 + 5 * 10, 40)),
        (Point::new(24 + 6 * 10, 40), Point::new(24 + 8 * 10, 40)),
    ];

    let mut now = rtc_channel.receive().await;
    let mut prev_time = now;
    let mut set_pos = 0;
    
    // 初始渲染
    display.clear_buffer();
    render_display(
        &mut display,
        &now,
        set_pos,
        cursor_visible,
        &CURSOR_POSITIONS,
        year_month_day_style,
        hour_minute_second_style,
        weekday_style,
        &WEEKDAYS,
    );
    let _ = display.flush();

    loop {
        let mut needs_redraw = false;
        
        // 接收新时间
        if let Ok(new_time) = rtc_channel.try_receive() {
            now = new_time;
            needs_redraw = true;
        }
        
        // 接收新位置
        if let Ok(new_pos) = key_channel.try_receive() {
            set_pos = new_pos;
            needs_redraw = true;
        }
        
        // 更新光标闪烁状态
        if Instant::now() - last_blink_time >= BLINK_INTERVAL {
            cursor_visible = !cursor_visible;
            last_blink_time = Instant::now();
            needs_redraw = true;
        }
        
        // 只有需要时才重绘
        if needs_redraw || now != prev_time {
            display.clear_buffer();
            render_display(
                &mut display,
                &now,
                set_pos,
                cursor_visible,
                &CURSOR_POSITIONS,
                year_month_day_style,
                hour_minute_second_style,
                weekday_style,
                &WEEKDAYS,
            );
            
            // 更新物理显示
            display.flush().ok();
            prev_time = now;
        }

        ticker.next().await;
    }
}

/// 渲染显示内容
fn render_display(
    display: &mut impl DrawTarget<Color = BinaryColor>,
    now: &NaiveDateTime,
    set_pos: u8,
    cursor_visible: bool,
    cursor_positions: &[(Point, Point); 6],
    year_month_day_style: MonoTextStyle<'_, BinaryColor>,
    hour_minute_second_style: MonoTextStyle<'_, BinaryColor>,
    weekday_style: MonoTextStyle<'_, BinaryColor>,
    weekdays: &[&str; 7],
) {
    // 渲染日期
    let mut date_str = heapless::String::<16>::new();
    write!(
        &mut date_str,
        "{:04}-{:02}-{:02}",
        now.year(),
        now.month(),
        now.day()
    ).unwrap();
    
    Text::with_baseline(
        &date_str, 
        Point::new(24, 4),
        year_month_day_style, 
        Baseline::Top
    ).draw(display).ok();

    // 渲染时间
    let mut time_str = heapless::String::<16>::new();
    write!(
        &mut time_str,
        "{:02}:{:02}:{:02}",
        now.hour(),
        now.minute(),
        now.second()
    ).unwrap();
    
    Text::with_baseline(
        &time_str, 
        Point::new(24, 21), 
        hour_minute_second_style, 
        Baseline::Top
    ).draw(display).ok();

    // 渲染星期
    let weekday_str = weekdays[now.weekday().num_days_from_monday() as usize];
    let text_width = weekday_str.len() * 8;
    let x_pos = (128 - text_width) / 2;
    Text::with_baseline(
        weekday_str, 
        Point::new(x_pos as i32, 46), 
        weekday_style, 
        Baseline::Top
    ).draw(display).ok();

    // 渲染光标
    if cursor_visible && set_pos != 0 && set_pos <= 6 {
        let (start, end) = cursor_positions[set_pos as usize - 1];
        Line::new(start, end)
            .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
            .draw(display)
            .ok();
    }
}

#[embassy_executor::task]
async fn rtc_update(
    rtc_sender: Sender<'static, ThreadModeRawMutex, NaiveDateTime, 1>, 
    delay: embassy_time::Duration
) {
    let mut now = NaiveDate::from_ymd_opt(2025, 7, 20)
        .unwrap()
        .and_hms_opt(18, 00, 00)
        .unwrap();

    let mut ticker = Ticker::every(delay);
    let mut last_update = Instant::now();
    const RTC_UPDATE_INTERVAL: embassy_time::Duration = embassy_time::Duration::from_secs(1);

    loop {
        // 正常时间推进（每秒更新一次）
        if Instant::now() - last_update >= RTC_UPDATE_INTERVAL {
            now = now.checked_add_signed(chrono::Duration::seconds(1))
                .unwrap_or(now);
            last_update = Instant::now();
        }
        
        // 发送更新
        rtc_sender.send(now).await;
        
        ticker.next().await;
    }
}

#[embassy_executor::task]
async fn key_update(
    mut button: ExtiInput<'static>,
    key_sender: Sender<'static, ThreadModeRawMutex, u8, 1>,
    debounce_delay: embassy_time::Duration
) {
    let mut current_mode: u8 = 0;

    loop {
        button.wait_for_falling_edge().await;
        Timer::after(debounce_delay).await;
        
        if button.is_high() {
            continue;
        }
        
        current_mode = (current_mode + 1) % 7;
        key_sender.send(current_mode).await;
        
        button.wait_for_rising_edge().await;
    }
}