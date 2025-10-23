use chrono::{Datelike, NaiveDate, NaiveDateTime, Timelike, TimeZone, Utc};
use core::fmt::Write;
use embassy_stm32::{
    flash::Flash,
    i2c::{self, Master},
    peripherals,
};
use embedded_graphics::{
    mono_font::{ascii::FONT_8X13, ascii::FONT_10X20, MonoTextStyle},
    pixelcolor::BinaryColor,
    prelude::*,
    primitives::{Line, PrimitiveStyle},
    text::{Baseline, Text},
};
use embassy_sync::{
    blocking_mutex::raw::ThreadModeRawMutex,
    channel::{Receiver, Sender},
};
use embassy_time::{Ticker, Timer, Instant};
use ssd1306::{prelude::*, I2CDisplayInterface, Ssd1306};
use embassy_stm32::Peri;
use embassy_stm32::flash::Blocking;
use crate::FlashError;

const MAGIC_NUMBER: u32 = 0xaa55aa55; // 魔数用于验证数据有效性

/// 从Flash读取时间
pub fn read_time_from_flash(_flash: &mut Flash<'_, Blocking>, addr: u32) -> Option<NaiveDateTime> {
    // 读取魔数验证数据有效性
    let magic = unsafe { read_volatile_u32(addr as *const u32) };
    if magic != MAGIC_NUMBER {
        return None; // 数据无效或未初始化
    }

    // 读取时间戳（u64格式）
    let timestamp_lo = (unsafe { read_volatile_u32((addr + 4) as *const u32) }) as u64;
    let timestamp_hi = (unsafe { read_volatile_u32((addr + 8) as *const u32) }) as u64;
    let timestamp = timestamp_lo | (timestamp_hi << 32);

    // 将Unix时间戳转换为NaiveDateTime
    match Utc.timestamp_opt(timestamp as i64, 0) {
        chrono::LocalResult::Single(datetime) => Some(datetime.naive_utc()),
        _ => None,
    }
}

/// 保存时间到Flash
pub fn save_time_to_flash(
    flash: &mut Flash<'_, Blocking>,
    addr: u32,
    time: NaiveDateTime
) -> Result<(), FlashError> {
    // 将NaiveDateTime转换为Unix时间戳
    let timestamp = time.and_utc().timestamp() as u64;

    // 准备写入数据
    let data: [u32; 3] = [
        MAGIC_NUMBER, // 魔数验证
        (timestamp & 0xffff_ffff) as u32, // 时间戳低32位
        ((timestamp >> 32) & 0xffff_ffff) as u32, // 时间戳高32位
    ];

    // 将u32数据转换为u8字节序列
    let bytes: &[u8] = unsafe {
        core::slice::from_raw_parts(data.as_ptr() as *const u8, core::mem::size_of_val(&data))
    };

    // Flash操作：擦除→写入→验证
    // 注意：Embassy STM32的Flash API会自动处理解锁和锁定

    // 1. 擦除目标页
    if flash.blocking_erase(addr, addr + 1024).is_err() {
        return Err(FlashError::EraseFailed);
    }

    // 2. 写入数据
    if flash.blocking_write(addr, bytes).is_err() {
        return Err(FlashError::WriteFailed);
    }

    // 3. 验证写入
    for (i, &expected_byte) in bytes.iter().enumerate() {
        let verified = unsafe { read_volatile_u8((addr + (i as u32)) as *const u8) };
        if verified != expected_byte {
            return Err(FlashError::VerificationFailed);
        }
    }

    Ok(())
}

// volatile读取函数（读u32）
unsafe fn read_volatile_u32(addr: *const u32) -> u32 {
    unsafe { core::ptr::read_volatile(addr) }
}

// volatile读取函数（读u8）
unsafe fn read_volatile_u8(addr: *const u8) -> u8 {
    unsafe { core::ptr::read_volatile(addr) }
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
    weekdays: &[&str; 7]
) {
    // 渲染日期
    let mut date_str = heapless::String::<16>::new();
    write!(&mut date_str, "{:04}-{:02}-{:02}", now.year(), now.month(), now.day()).unwrap();

    Text::with_baseline(&date_str, Point::new(24, 4), year_month_day_style, Baseline::Top)
        .draw(display)
        .ok();

    // 渲染时间
    let mut time_str = heapless::String::<16>::new();
    write!(&mut time_str, "{:02}:{:02}:{:02}", now.hour(), now.minute(), now.second()).unwrap();

    Text::with_baseline(&time_str, Point::new(24, 21), hour_minute_second_style, Baseline::Top)
        .draw(display)
        .ok();

    // 渲染星期
    let weekday_str = weekdays[now.weekday().num_days_from_monday() as usize];
    let text_width = weekday_str.len() * 8;
    let x_pos = (128 - text_width) / 2;
    Text::with_baseline(weekday_str, Point::new(x_pos as i32, 46), weekday_style, Baseline::Top)
        .draw(display)
        .ok();

    // 渲染光标
    if cursor_visible && set_pos != 0 && set_pos <= 6 {
        let (start, end) = cursor_positions[(set_pos as usize) - 1];
        Line::new(start, end)
            .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
            .draw(display)
            .ok();
    }
}

#[embassy_executor::task]
pub async fn rtc_update(
    flash_peripheral: Peri<'static, peripherals::FLASH>,
    rtc_sender: Sender<'static, ThreadModeRawMutex, NaiveDateTime, 1>,
    delay: embassy_time::Duration
) {
    // 获取Flash实例
    let mut flash = Flash::new_blocking(flash_peripheral);

    // Flash存储地址定义 - 使用最后1KB页（避开程序区域）
    const FLASH_STORAGE_ADDR: u32 = 0x0800f800;

    // 1. 上电后，尝试从Flash读取保存的时间
    let mut now = read_time_from_flash(&mut flash, FLASH_STORAGE_ADDR).unwrap_or_else(|| {
        // 如果读取失败（如第一次启动），使用默认时间
        NaiveDate::from_ymd_opt(2025, 10, 23).unwrap().and_hms_opt(19, 00, 00).unwrap()
    });

    let mut ticker = Ticker::every(delay);
    let mut last_update = Instant::now();
    let mut last_save = Instant::now();
    const RTC_UPDATE_INTERVAL: embassy_time::Duration = embassy_time::Duration::from_secs(1);
    const SAVE_INTERVAL: embassy_time::Duration = embassy_time::Duration::from_secs(60); // 每60秒保存一次

    loop {
        if Instant::now() - last_update >= RTC_UPDATE_INTERVAL {
            now = now.checked_add_signed(chrono::Duration::seconds(1)).unwrap_or(now);
            last_update = Instant::now();
        }

        // 2. 定期保存当前时间到Flash
        if Instant::now() - last_save >= SAVE_INTERVAL {
            if let Err(_e) = save_time_to_flash(&mut flash, FLASH_STORAGE_ADDR, now) {
                // 处理保存错误，例如记录日志
            } else {
                last_save = Instant::now();
            }
        }

        rtc_sender.send(now).await;
        ticker.next().await;
    }
}


#[embassy_executor::task]
pub async fn oled_display(
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

    let year_month_day_style = MonoTextStyle::new(&FONT_8X13, BinaryColor::On);

    let hour_minute_second_style = MonoTextStyle::new(&FONT_10X20, BinaryColor::On);

    let weekday_style = MonoTextStyle::new(&FONT_8X13, BinaryColor::On);

    // 星期字符串
    const WEEKDAYS: [&str; 7] = [
        "Monday",
        "Tuesday",
        "Wednesday",
        "Thursday",
        "Friday",
        "Saturday",
        "Sunday",
    ];

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
        &WEEKDAYS
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
                &WEEKDAYS
            );

            // 更新物理显示
            display.flush().ok();
            prev_time = now;
        }

        ticker.next().await;
    }
}
