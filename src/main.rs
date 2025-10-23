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
//! 6.	VS1053循环播放8字节音乐

#![no_std] // 不使用标准库
#![no_main] // 不使用标准main函数

use chrono::NaiveDateTime; // 合并chrono导入
use defmt_rtt as _;
use embassy_executor::Spawner;
use embassy_stm32::{
    gpio::{ Output, Speed, Level, Pull },
    i2c,
    time::Hertz,
    exti::ExtiInput
};
use embassy_sync::{
    blocking_mutex::raw::ThreadModeRawMutex,
    channel::Channel,
};
use embassy_time::Ticker;
use panic_probe as _;
use embassy_stm32::bind_interrupts;
use embassy_stm32::peripherals;
use crate::i2c::ErrorInterruptHandler;
use crate::i2c::EventInterruptHandler;

pub mod hardware;
pub mod vs1053;
pub mod task;


// 通道
static RTC_CHANNEL: Channel<ThreadModeRawMutex, NaiveDateTime, 1> = Channel::new();
static KEY_CHANNEL: Channel<ThreadModeRawMutex, u8, 1> = Channel::new();


#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let p = embassy_stm32::init(Default::default());
		bind_interrupts!(struct Irqs {
        I2C1_EV => EventInterruptHandler<peripherals::I2C1>;
        I2C1_ER => ErrorInterruptHandler<peripherals::I2C1>;
    });

    let mut config = i2c::Config::default();
    config.frequency = Hertz(400_000);

    let i2c = i2c::I2c::new(p.I2C1, p.PB6, p.PB7, Irqs, p.DMA1_CH6, p.DMA1_CH7, config);

    let key_exti = ExtiInput::new(p.PB15, p.EXTI15, Pull::Up);

    spawner
        .spawn(
            task::screen::oled_display(
                i2c,
                RTC_CHANNEL.receiver(),
                KEY_CHANNEL.receiver(),
                embassy_time::Duration::from_millis(200)
            )
        )
        .unwrap();

    spawner
        .spawn(task::screen::rtc_update(p.FLASH, RTC_CHANNEL.sender(), embassy_time::Duration::from_millis(20)))
        .unwrap();

    spawner
        .spawn(task::led::key_update(key_exti, KEY_CHANNEL.sender(), embassy_time::Duration::from_millis(15)))
        .unwrap();

    let mut led = Output::new(p.PC13, Level::High, Speed::Low);
    let mut ticker = Ticker::every(embassy_time::Duration::from_millis(500));

    loop {
        led.set_low();
        ticker.next().await;
        led.set_high();
        ticker.next().await;
    }
}




// 错误类型定义
#[derive(Debug, defmt::Format)]
pub enum FlashError {
    EraseFailed,
    WriteFailed,
    VerificationFailed,
    InvalidData,
}

