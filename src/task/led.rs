// 必要的系统导入
use defmt_rtt as _;
use panic_probe as _;

// embassy 相关（只保留使用的组件）
use embassy_stm32::exti::ExtiInput;
use embassy_sync::{
    blocking_mutex::raw::ThreadModeRawMutex,
    channel::Sender,
};
use embassy_time::Timer;


#[embassy_executor::task]
pub async fn key_update(
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