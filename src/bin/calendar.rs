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

use chrono::{Datelike, NaiveDate, NaiveDateTime, Timelike, Weekday};
use defmt_rtt as _; // 用于嵌入式日志记录
use embassy_executor::Spawner; // Embassy任务生成器
use embassy_stm32::{
    bind_interrupts,
    gpio::{Output, Speed, Level, Pull},
    i2c::{self, EventInterruptHandler, ErrorInterruptHandler, Master},
    peripherals,
    time::Hertz,
    exti::ExtiInput,
    timer::qei::{Qei, QeiPin},
};
use embedded_graphics::{
    mono_font::{ascii::FONT_8X13, ascii::FONT_10X20, MonoTextStyleBuilder},
    pixelcolor::BinaryColor,
    primitives::{Line, PrimitiveStyle},
    prelude::*,
    text::{Baseline, Text},
};
use embassy_sync::{
    blocking_mutex::raw::ThreadModeRawMutex,
    channel::{Channel, Receiver, Sender},
};
use embassy_time::{Ticker, Timer};
use panic_probe as _; // 用于panic处理
use ssd1306::{prelude::*, I2CDisplayInterface, Ssd1306};
use heapless::String; // 堆栈分配字符串
use core::fmt::Write; // 格式化输出

// 用于任务间共享RTC数据的通道
static RTC_CHANNEL: Channel<ThreadModeRawMutex, NaiveDateTime, 2> = Channel::new();

// 用于传递旋转编码器变化值的通道
static ARE_CHANNEL: Channel<ThreadModeRawMutex, i32, 3> = Channel::new();

// 用于按钮按下事件（字段选择）的通道
static KEY_CHANNEL: Channel<ThreadModeRawMutex, i32, 1> = Channel::new();

/// 主应用程序入口点
#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    // 使用默认配置初始化外设
    let p = embassy_stm32::init(Default::default());

    // 绑定I2C中断处理函数
    bind_interrupts!(struct Irqs {
        I2C1_EV => EventInterruptHandler<peripherals::I2C1>;
        I2C1_ER => ErrorInterruptHandler<peripherals::I2C1>;
    });
    
    // 配置I2C参数
    let mut config = i2c::Config::default();
    config.frequency = Hertz(400_000); // 400kHz通信速率
    
    // 初始化I2C外设
    let i2c = i2c::I2c::new(
        p.I2C1,    // I2C1外设
        p.PB6,     // SCL引脚
        p.PB7,     // SDA引脚
        Irqs,      // 中断处理
        p.DMA1_CH6, // DMA通道6
        p.DMA1_CH7, // DMA通道7
        config,    // 配置参数
    );

    // 配置旋转编码器（使用TIM1正交编码接口）
    let encoder = Qei::new(
        p.TIM1,    // 定时器1
        QeiPin::new(p.PA8), // CLK引脚
        QeiPin::new(p.PA9)  // DT引脚
    );

    // 配置按钮（使用外部中断，上拉配置）
    let key_exti = ExtiInput::new(p.PB15, p.EXTI15, Pull::Up);

    // 生成OLED显示任务
    _spawner.spawn(oled_display(
        i2c, 
        RTC_CHANNEL.receiver(), 
        KEY_CHANNEL.receiver(),
        embassy_time::Duration::from_millis(100) // 100ms刷新周期
    )).unwrap();

    // 生成RTC更新时间任务
    _spawner.spawn(rtc_update(
        RTC_CHANNEL.sender(), 
        KEY_CHANNEL.receiver(),
        ARE_CHANNEL.receiver(),
        embassy_time::Duration::from_millis(30) // 30ms更新周期
    )).unwrap();

    // 生成旋转编码器处理任务
    _spawner.spawn(are_update(
        encoder, 
        ARE_CHANNEL.sender(),
        embassy_time::Duration::from_millis(100) // 100ms检测周期
    )).unwrap();

    // 生成按钮处理任务
    _spawner.spawn(key_update(
        key_exti, 
        KEY_CHANNEL.sender(),
        embassy_time::Duration::from_millis(10) // 10ms消抖延迟
    )).unwrap();

    // 配置板载LED（PC13）作为心跳指示灯
    let mut led = Output::new(p.PC13, Level::High, Speed::Low);
    let mut ticker = Ticker::every(embassy_time::Duration::from_millis(500));
    
    // 主心跳循环
    loop {
        led.set_low();  // LED亮
        ticker.next().await;
        led.set_high(); // LED灭
        ticker.next().await;
    }
}

/// OLED显示渲染任务
#[embassy_executor::task]
async fn oled_display(
    i2c: i2c::I2c<'static, embassy_stm32::mode::Async, Master>,
    rtc_channel: Receiver<'static, ThreadModeRawMutex, NaiveDateTime, 2>, 
    key_channel: Receiver<'static, ThreadModeRawMutex, i32, 1>,
    delay: embassy_time::Duration
) {
    let mut ticker = Ticker::every(delay);
    ticker.next().await; // 等待第一个tick

    // 初始化显示接口和控制器
    let interface = I2CDisplayInterface::new(i2c);
    let mut display = Ssd1306::new(
        interface, 
        DisplaySize128x64, // 128x64分辨率
        DisplayRotation::Rotate0 // 不旋转
    ).into_buffered_graphics_mode();
    
    // 初始化OLED显示
    match display.init() {
        Ok(_) => None,
        Err(_e) => {
             core::prelude::v1::Some(loop {

            })
            // 进入错误处理状态
        }
    };

    // 配置文本渲染样式
    let year_month_day_style = MonoTextStyleBuilder::new()
        .font(&FONT_8X13)          // 日期使用8x13字体
        .text_color(BinaryColor::On) // 白色显示
        .build();

    let hour_minute_second_style = MonoTextStyleBuilder::new()
        .font(&FONT_10X20)          // 时间使用10x20字体
        .text_color(BinaryColor::On) 
        .build();

    let weekday_style = MonoTextStyleBuilder::new()
        .font(&FONT_8X13)           // 星期使用8x13字体
        .text_color(BinaryColor::On)
        .build();

    // 光标状态管理
    let mut cursor_visible = false; // 当前光标可见状态
    let mut last_blink_time = embassy_time::Instant::now(); // 上次闪烁时间
    const BLINK_INTERVAL: embassy_time::Duration = embassy_time::Duration::from_millis(500); // 500ms闪烁间隔

    // 光标位置定义（对应6个可调整字段）
    const CURSOR_POSITIONS: [(Point, Point); 6] = [
        // 年字段位置 (起点, 终点)
        (Point::new(24, 18), Point::new(24 + 4 * 8, 18)),
        // 月字段位置
        (Point::new(24 + 5 * 8, 18), Point::new(24 + 7 * 8, 18)),
        // 日字段位置
        (Point::new(24 + 8 * 8, 18), Point::new(24 + 10 * 8, 18)),
        // 小时字段位置
        (Point::new(24, 40), Point::new(24 + 2 * 10, 40)),
        // 分钟字段位置
        (Point::new(24 + 3 * 10, 40), Point::new(24 + 5 * 10, 40)),
        // 秒钟字段位置
        (Point::new(24 + 6 * 10, 40), Point::new(24 + 8 * 10, 40)),
    ];

    // 初始化当前时间和选择位置
    let mut now = rtc_channel.receive().await;
    let mut set_pos = 0; // 当前选择的字段（0表示无选择）

    loop {
        display.clear_buffer(); // 清空显示缓冲区

        // 更新光标闪烁状态
        if embassy_time::Instant::now() - last_blink_time >= BLINK_INTERVAL {
            cursor_visible = !cursor_visible; // 切换可见状态
            last_blink_time = embassy_time::Instant::now();
        }

        // 尝试接收新的时间数据（非阻塞）
        if let Ok(new_time) = rtc_channel.try_receive() {
            now = new_time;
        }

        // 尝试获取新的选择位置（非阻塞）
        if let Ok(new_pos) = key_channel.try_peek() {
            set_pos = new_pos;
        }

        // 如果需要绘制光标
        if cursor_visible && set_pos != 0 && set_pos <= 6 {
            let (start, end) = CURSOR_POSITIONS[set_pos as usize - 1];
            // 在选中字段下方绘制横线
            Line::new(start, end)
                .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
                .draw(&mut display)
                .unwrap();
        }

        // 渲染日期（YYYY-MM-DD格式）
        let mut date_buf: String<10> = String::new();
        write!(&mut date_buf, "{:04}-{:02}-{:02}", now.year(), now.month(), now.day()).unwrap();
        Text::with_baseline(
            &date_buf, 
            Point::new(24, 4), // 屏幕位置
            year_month_day_style, 
            Baseline::Top
        ).draw(&mut display).unwrap();

        // 渲染时间（HH:MM:SS格式）
        let mut time_buf: String<8> = String::new();
        write!(&mut time_buf, "{:02}:{:02}:{:02}", now.hour(), now.minute(), now.second()).unwrap();
        Text::with_baseline(
            &time_buf, 
            Point::new(24, 21), 
            hour_minute_second_style, 
            Baseline::Top
        ).draw(&mut display).unwrap();

        // 渲染星期
        let weekday_str = match now.weekday() {
            Weekday::Mon => "星期一",
            Weekday::Tue => "星期二",
            Weekday::Wed => "星期三",
            Weekday::Thu => "星期四",
            Weekday::Fri => "星期五",
            Weekday::Sat => "星期六",
            Weekday::Sun => "星期日",  
        };
        let text_width = weekday_str.len() * 8; // 计算文本宽度
        let x_pos = (128 - text_width) / 2;    // 居中计算
        Text::with_baseline(
            weekday_str, 
            Point::new(x_pos as i32, 46), 
            weekday_style, 
            Baseline::Top
        ).draw(&mut display).unwrap();

        // 更新物理显示
        display.flush().unwrap();

        // 等待下一个渲染周期
        ticker.next().await;
    }
}

/// 软件RTC管理任务
///
/// 主要职责：
/// 1. 维护虚拟实时时钟
/// 2. 处理旋转编码器的时间调整
/// 3. 管理字段选择状态
#[embassy_executor::task]
async fn rtc_update(
    rtc_sender: Sender<'static, ThreadModeRawMutex, NaiveDateTime, 2>, 
    key_receiver: Receiver<'static, ThreadModeRawMutex, i32, 1>, 
    are_receiver: Receiver<'static, ThreadModeRawMutex, i32, 3>, 
    delay: embassy_time::Duration
) {
    // 初始化特定日期时间（2025-07-20 18:00:00）
    let mut now = NaiveDate::from_ymd_opt(2025, 7, 20)
        .unwrap()
        .and_hms_opt(18, 00, 00)
        .unwrap();

    let mut ticker = Ticker::every(delay);
    let mut set_pos: i32 = 0; // 当前选择的字段
    let mut prev_time = now;   // 用于变化检测

    loop {
        // 检查字段选择变化
        if let Ok(new_pos) = key_receiver.try_peek() {
            set_pos = new_pos;
        }

        // 根据选中字段应用旋转编码器的调整
        if set_pos != 0 {
            if let Ok(delta) = are_receiver.try_receive() {
                now = match set_pos {
                    1 => now.checked_add_signed(chrono::Duration::days(365 * delta as i64))
                        .unwrap_or(now), // 调整年份
                    2 => now.checked_add_signed(chrono::Duration::days(30 * delta as i64))
                        .unwrap_or(now), // 调整月份
                    3 => now.checked_add_signed(chrono::Duration::days(delta as i64))
                        .unwrap_or(now), // 调整日期
                    4 => now.checked_add_signed(chrono::Duration::hours(delta as i64))
                        .unwrap_or(now), // 调整小时
                    5 => now.checked_add_signed(chrono::Duration::minutes(delta as i64))
                        .unwrap_or(now), // 调整分钟
                    6 => now.checked_add_signed(chrono::Duration::seconds(delta as i64))
                        .unwrap_or(now), // 调整秒钟
                    _ => now,
                };
            }
        } else {
            // 正常时间推进
            now = now.checked_add_signed(chrono::Duration::milliseconds(delay.as_millis() as i64))
                .unwrap_or(now);
        }
        
        // 当时间变化时广播更新
        if prev_time != now {
            rtc_sender.clear(); // 清空通道
            rtc_sender.send(now).await; // 发送新时间
            prev_time = now; // 更新记录
        }

        ticker.next().await; // 等待下一个周期
    }
}

/// 旋转编码器处理任务
///
/// 主要职责：
/// 1. 读取编码器位置变化
/// 2. 处理计数器溢出/下溢
/// 3. 应用平滑滤波
/// 4. 广播相对变化值
#[embassy_executor::task]
async fn are_update(
    encoder: Qei<'static, peripherals::TIM1>,
    are_sender: Sender<'static, ThreadModeRawMutex, i32, 3>, 
    delay: embassy_time::Duration
) {
    let mut ticker = Ticker::every(delay);
    let mut prev_count = encoder.count(); // 上次编码器位置
    let mut accumulated_delta = 0;       // 累积的变化值
    const SMOOTHING_FACTOR: i32 = 4;     // 平滑因子（灵敏度调整）

    // 发送初始化信号
    are_sender.send(0).await;

    loop {
        let curr_count = encoder.count(); // 当前编码器位置
        let raw_delta = curr_count as i32 - prev_count as i32; // 原始变化值
        
        // 处理16位计数器溢出
        let adjusted_delta = if raw_delta > 32767 {
            raw_delta - 65536  // 正向溢出校正
        } else if raw_delta < -32768 {
            raw_delta + 65536  // 负向溢出校正
        } else {
            raw_delta
        };
        
        // 累积小幅度变化
        accumulated_delta -= adjusted_delta;
        
        // 当累积变化达到阈值时发送有效变化
        if accumulated_delta.abs() >= SMOOTHING_FACTOR {
            are_sender.send(accumulated_delta / SMOOTHING_FACTOR).await;
            accumulated_delta %= SMOOTHING_FACTOR; // 保留余数
        }
        
        prev_count = curr_count; // 更新位置记录
        ticker.next().await;     // 等待下一个周期
    }
}

/// 按钮处理任务
///
/// 主要职责：
/// 1. 检测按钮按下（带消抖）
/// 2. 循环切换设置模式（年→月→日→时→分→秒→正常）
/// 3. 广播模式变化
#[embassy_executor::task]
async fn key_update(
    mut button: ExtiInput<'static>,
    key_sender: Sender<'static, ThreadModeRawMutex, i32, 1>,
    debounce_delay: embassy_time::Duration
) {
    let mut current_mode = 0; // 0=正常模式, 1-6=设置模式

    loop {
        // 等待按钮按下（下降沿）
        button.wait_for_falling_edge().await;
        
        // 消抖延迟
        Timer::after(debounce_delay).await;
        
        // 验证按钮是否仍被按下（防抖动）
        if button.is_high() {
            continue;
        }
        
        // 循环切换模式（0 → 1 → 2 → 3 → 4 → 5 → 6 → 0）
        current_mode = (current_mode + 1) % 7;
        
        // 广播新模式
        key_sender.clear();
        key_sender.send(current_mode).await;
        
        // 等待按钮释放
        button.wait_for_rising_edge().await;
    }
}