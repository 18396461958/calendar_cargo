// hardware.rs 或 vs1053.rs
use embassy_stm32::{
    gpio::{Output, Input, Level, Pull, Speed},
    spi,
    time::Hertz,
    peripherals::{
        SPI1, PA5, PA7, PA6, DMA1_CH3, DMA1_CH2,
        PA4, PA3, PA2, PA1
    }
};
use embassy_stm32::mode::Async;

pub struct VS1053Hardware {
    pub spi: spi::Spi<'static, Async>,
    pub cs: Output<'static>,
    pub dc: Output<'static>, 
    pub req: Input<'static>,
    pub reset: Output<'static>,
}
 use embassy_stm32::Peri;
impl VS1053Hardware {
    pub fn init(
        spi1: Peri<'static, SPI1>,
        sck: Peri<'static, PA5>,
        mosi: Peri<'static, PA7>,
        miso: Peri<'static, PA6>,
        dma_tx: Peri<'static, DMA1_CH3>,
        dma_rx: Peri<'static, DMA1_CH2>,
        cs_pin: Peri<'static, PA4>,
        dc_pin: Peri<'static, PA3>,
        req_pin: Peri<'static, PA2>,
        reset_pin: Peri<'static, PA1>,
    ) -> Result<Self, embassy_stm32::spi::Error> {
        // SPI配置 - VS1053需要特定时序
        let mut spi_config = spi::Config::default();
        spi_config.frequency = Hertz(250_000); // 初始化时用低速，后续可提高
        spi_config.mode = embassy_stm32::spi::Mode {
            polarity: embassy_stm32::spi::Polarity::IdleLow,
            phase: embassy_stm32::spi::Phase::CaptureOnFirstTransition,
        };
        
        // 初始化SPI
        let spi = spi::Spi::new(
            spi1,
            sck,   // SCK
            mosi,  // MOSI
            miso,  // MISO  
            dma_tx,
            dma_rx,
            spi_config,
        );
        
        // VS1053控制引脚
        let cs = Output::new(cs_pin, Level::High, Speed::Low);    // 片选
        let dc = Output::new(dc_pin, Level::High, Speed::Low);    // 数据/命令选择  
        let req = Input::new(req_pin, Pull::Up);                   // 数据请求
        let reset = Output::new(reset_pin, Level::High, Speed::Low); // 复位
        
        Ok(Self { spi, cs, dc, req, reset })
    }
}