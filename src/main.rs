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


// 音频命令枚举
#[derive(defmt::Format, Clone, Copy)]
pub enum AudioCommand {
    Play8BitMusic,
    Stop,
    SetVolume(u8), // 0-100
}


// 通道
static RTC_CHANNEL: Channel<ThreadModeRawMutex, NaiveDateTime, 1> = Channel::new();
static KEY_CHANNEL: Channel<ThreadModeRawMutex, u8, 1> = Channel::new();
static AUDIO_CHANNEL: Channel<ThreadModeRawMutex, AudioCommand, 4> = Channel::new();


#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let p = embassy_stm32::init(Default::default());
    let vs1053_hw = vs1053::vs1053_stm32::VS1053Hardware::init(
        p.SPI1,
        p.PA5,
        p.PA7,
        p.PA6,
        p.DMA1_CH3,
        p.DMA1_CH2,
        p.PA4,
        p.PA3,
        p.PA2,
        p.PA1,
    ).expect("VS1053硬件初始化失败");
		bind_interrupts!(struct Irqs {
        I2C1_EV => EventInterruptHandler<peripherals::I2C1>;
        I2C1_ER => ErrorInterruptHandler<peripherals::I2C1>;
    });
    // 初始化VS1053驱动
    let mut vs1053_driver = vs1053::lib::VS1053::new(
        vs1053_hw.spi,
        vs1053_hw.cs,
        vs1053_hw.dc, 
        vs1053_hw.req,
        embassy_time::Delay,

    );
     match vs1053_driver.init() {
        Ok(_) => {
            defmt::info!("VS1053初始化成功");
            
            // 设置MP3模式并加载补丁[10](@ref)
            if let Err(e) = vs1053_driver.set_mp3_mode_on() {
                defmt::warn!("设置MP3模式失败: {:?}", e);
            }
            
            // 加载默认补丁（修复bug和增强功能）[10](@ref)
            if vs1053_driver.get_chip_version().unwrap_or(0) == 4 {
                let _ = vs1053_driver.load_default_patches();
            }
            
            // 设置初始音量[9](@ref)
            let _ = vs1053_driver.set_volume(80); // 80%音量
        }
        Err(e) => {
            defmt::error!("VS1053初始化失败: {:?}", e);
        }
    }

    // 启动音频播放任务
    spawner
        .spawn(task::mp3::audio_player_task(
            vs1053_driver,
            AUDIO_CHANNEL.receiver(),
        ))
        .unwrap();

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

