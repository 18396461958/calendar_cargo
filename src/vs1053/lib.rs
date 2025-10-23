//! # VS1053
//!
//! ## Overview
//!
//! SPI driver for generic VS1053 breakout board
//!
//! ## Usage
//! [example]: https://gitlab.com/esp322054205/vs1053/-/tree/master/examples/play_mp3
//!

use embedded_hal::delay::DelayNs;
use embedded_hal::digital::{ InputPin, OutputPin };
use embedded_hal::spi::SpiBus;
use crate::vs1053::vs1053b_patches;
use crate::vs1053::vs1053b_patches_flac;

pub mod regs {
    pub const SCI_MODE: u8 = 0x0;
    pub const SCI_STATUS: u8 = 0x1;
    pub const SCI_BASS: u8 = 0x2;
    pub const SCI_CLOCKF: u8 = 0x3;
    pub const SCI_DECODE_TIME: u8 = 0x4;
    pub const SCI_AUDATA: u8 = 0x5;
    pub const SCI_WRAM: u8 = 0x6;
    pub const SCI_WRAMADDR: u8 = 0x7;
    pub const SCI_HDAT0: u8 = 0x8;
    pub const SCI_HDAT1: u8 = 0x9;
    pub const SCI_AIADDR: u8 = 0xa;
    /// maximum volume is 0x0000 and total silence is 0xFEFE
    /// Setting SCI_VOL to 0xFFFF will activate analog powerdown mode
    pub const SCI_VOL: u8 = 0xb;
    pub const SCI_AICTRL0: u8 = 0xc;
    pub const SCI_AICTRL1: u8 = 0xd;
    pub const SCI_AICTRL2: u8 = 0xe;
    pub const SCI_AICTRL3: u8 = 0xf;
    pub const SCI_NUM_REGISTERS: u8 = 0xf;
}

pub mod sci_mode {
    pub const SM_DIFF: u8 = 0;
    pub const SM_LAYER12: u8 = 1;
    pub const SM_RESET: u8 = 2;
    pub const SM_CANCEL: u8 = 3;
    pub const SM_EARSPEAKER_LO: u8 = 4;
    pub const SM_TESTS: u8 = 5;
    pub const SM_STREAM: u8 = 6;
    pub const SM_EARSPEAKER_HI: u8 = 7;
    pub const SM_DACT: u8 = 8;
    pub const SM_SDIORD: u8 = 9;
    pub const SM_SDISHARE: u8 = 10;
    pub const SM_SDINEW: u8 = 11;
    pub const SM_ADPCM: u8 = 12;
    pub const SM_ADCPM_HP: u8 = 13;
    pub const SM_LINE1: u8 = 14;
    pub const SM_CLK_RANGE: u8 = 15;
}

const VS1053_CHUNK_SIZE: u8 = 32;
const END_FILL_BYTE: u16 = 0x1e06;

const ADDR_REG_GPIO_DDR_RW: u16 = 0xc017;
const ADDR_REG_GPIO_ODATA_RW: u16 = 0xc019;

/// 0-100% volume API
pub const VOLUME_MAX: u8 = 100;
pub const VOLUME_MIN: u8 = 0;

/// raw volume API
pub const RAW_VOLUME_MAX: u16 = 0;
pub const RAW_VOLUME_MIN: u16 = 0xfefe;
pub const RAW_VOLUME_OFF: u16 = 0xffff;

const END_FILL_BYTE_BUF_LEN: u16 = 2052;

fn map_volume(x: u8, in_min: i16, in_max: i16, out_min: i16, out_max: i16) -> u8 {
    ((((x as i16) - in_min) * (out_max - out_min + 1)) / (in_max - in_min + 1) + out_min) as u8
}
use defmt::Format;
#[derive(Clone, Copy, Debug,Format)]
pub enum VS1053Error<BUS, CS, DC> {
    Spi(BUS),
    Cs(CS),
    Dc(DC),
    InitError,
    StopError,
}

#[derive(Clone, Copy, Debug)]
pub struct VS1053<BUS, CS, DC, REQ, DELAY> {
    spi: BUS,
    cs: CS,
    dc: DC,
    req: REQ,
    delay: DELAY,
    end_fill_byte: u8,
}

impl<BUS, CS, DC, REQ, DELAY> VS1053<BUS, CS, DC, REQ, DELAY>
    where BUS: SpiBus, CS: OutputPin, DC: OutputPin, REQ: InputPin, DELAY: DelayNs
{
    // 移除关联类型定义
    // type ErrorType = VS1053Error<BUS::Error, CS::Error, DC::Error>;

    pub fn new(spi: BUS, cs: CS, dc: DC, req: REQ, delay: DELAY) -> Self {
        Self {
            spi,
            cs,
            dc,
            req,
            delay,
            end_fill_byte: 0,
        }
    }

    fn await_data_request(&mut self) {
        // TODO - brain dead stupid busy wait
        while self.req.is_low().unwrap() {
            self.delay.delay_us(100);
        }
    }

    /// can be used to external check if chip is ready to accept new data
    pub fn get_data_request(&mut self) -> bool {
        self.req.is_high().unwrap()
    }

    // 在所有方法中使用具体的错误类型
    fn control_mode_on(&mut self) -> Result<(), VS1053Error<BUS::Error, CS::Error, DC::Error>> {
        self.dc.set_high().map_err(VS1053Error::Dc)?;
        self.cs.set_low().map_err(VS1053Error::Cs)?;
        Ok(())
    }

    fn control_mode_off(&mut self) -> Result<(), VS1053Error<BUS::Error, CS::Error, DC::Error>> {
        self.cs.set_high().map_err(VS1053Error::Cs)?;
        Ok(())
    }

    fn data_mode_on(&mut self) -> Result<(), VS1053Error<BUS::Error, CS::Error, DC::Error>> {
        self.cs.set_high().map_err(VS1053Error::Cs)?;
        self.dc.set_low().map_err(VS1053Error::Dc)?;
        Ok(())
    }

    fn data_mode_off(&mut self) -> Result<(), VS1053Error<BUS::Error, CS::Error, DC::Error>> {
        self.dc.set_high().map_err(VS1053Error::Dc)?;
        Ok(())
    }

    fn read_register(&mut self, register: u8) -> Result<u16, VS1053Error<BUS::Error, CS::Error, DC::Error>> {
        let write_buffer = [3, register];
        let mut read_bufer: [u8; 2] = [0u8; 2];

        self.control_mode_on()?;
        self.spi.write(&write_buffer).map_err(VS1053Error::Spi)?;
        self.spi.read(&mut read_bufer).map_err(VS1053Error::Spi)?;
        let value = u16::from_be_bytes(read_bufer);

        self.await_data_request();
        self.control_mode_off()?;
        Ok(value)
    }

    fn write_register(&mut self, register: u8, value: u16) -> Result<(), VS1053Error<BUS::Error, CS::Error, DC::Error>> {
        let buf: [u8; 2] = u16::to_be_bytes(value);
        let write_buffer = [2, register, buf[0], buf[1]];

        self.control_mode_on()?;
        self.spi.write(&write_buffer).map_err(VS1053Error::Spi)?;
        self.spi.flush().map_err(VS1053Error::Spi)?;
        self.await_data_request();
        self.control_mode_off()?;
        Ok(())
    }

    pub fn init(&mut self) -> Result<(), VS1053Error<BUS::Error, CS::Error, DC::Error>> {
        // spi mode should be slow at this point or you will get errors
        self.dc.set_high().map_err(VS1053Error::Dc)?;
        self.cs.set_high().map_err(VS1053Error::Cs)?;

        self.delay.delay_ms(100);

        self.dc.set_low().map_err(VS1053Error::Dc)?;
        self.cs.set_low().map_err(VS1053Error::Cs)?;

        self.delay.delay_ms(500);

        self.dc.set_high().map_err(VS1053Error::Dc)?;
        self.cs.set_high().map_err(VS1053Error::Cs)?;

        self.delay.delay_ms(500);

        if !self.req.is_high().unwrap() {
            // most probably not connected correct
            return Err(VS1053Error::InitError);
        }

        self.write_register(regs::SCI_VOL, 0)?;
        self.write_register(regs::SCI_AUDATA, 44101)?;
        self.write_register(regs::SCI_CLOCKF, 6 << 12)?; // Normal clock settings multiplyer 3.0 = 12.2 MHz
        self.write_register(
            regs::SCI_MODE,
            (1 << sci_mode::SM_SDINEW) | (1 << sci_mode::SM_LINE1)
        )?;

        self.delay.delay_ms(10);
        self.await_data_request();

        if let Ok(value) = self.wram_read(END_FILL_BYTE) {
            self.end_fill_byte = (value & 0xff) as u8;
        }
        self.delay.delay_ms(100);
        Ok(())
    }

    ///
    pub fn write(&mut self, buf: &[u8]) -> Result<(), VS1053Error<BUS::Error, CS::Error, DC::Error>> {
        self.data_mode_on()?;

        for chunk in buf.chunks(VS1053_CHUNK_SIZE as usize) {
            self.await_data_request();

            self.spi.write(chunk).map_err(VS1053Error::Spi)?;
            self.spi.flush().map_err(VS1053Error::Spi)?;
        }

        self.data_mode_off()?;

        Ok(())
    }

    /// direct read of SCI_VOL register
    pub fn get_volume_raw(&mut self) -> Result<u16, VS1053Error<BUS::Error, CS::Error, DC::Error>> {
        self.read_register(regs::SCI_VOL)
    }

    /// direct write to SCI_VOL register
    /// can be used to turn audio output off
    pub fn set_volume_raw(&mut self, vol: u16) -> Result<(), VS1053Error<BUS::Error, CS::Error, DC::Error>> {
        self.write_register(regs::SCI_VOL, vol)
    }

    /// Set volume as % value. Both left and right.
    /// Input value is 0..100.  100 is the loudest.
    pub fn set_volume(&mut self, vol: u8) -> Result<(), VS1053Error<BUS::Error, CS::Error, DC::Error>> {
        let value_left = map_volume(vol, 0, 100, 0xfe, 0); // 0..100% to left channel
        let value_right = map_volume(vol, 0, 100, 0xfe, 0); // 0..100% to right channel

        let buf: [u8; 2] = [value_left, value_right];

        let value = u16::from_be_bytes(buf);
        self.set_volume_raw(value)
    }

    fn write_cancel(&mut self) -> Result<(), VS1053Error<BUS::Error, CS::Error, DC::Error>> {
        self.write_register(regs::SCI_MODE, (1 << sci_mode::SM_SDINEW) | (1 << sci_mode::SM_CANCEL))
    }

    fn read_cancel(&mut self) -> Result<u8, VS1053Error<BUS::Error, CS::Error, DC::Error>> {
        let value = self.read_register(regs::SCI_MODE)?;
        let res = value & (1 << sci_mode::SM_CANCEL);
        Ok(if res == (1 << sci_mode::SM_CANCEL) { 1 } else { 0 })
    }

    /// in theory this is needed if stop_play fails
    /// set_mp3_mode_on calls it too
    /// patches must be loaded again after a soft reset!
    pub fn soft_reset(&mut self) -> Result<(), VS1053Error<BUS::Error, CS::Error, DC::Error>> {
        self.write_register(regs::SCI_MODE, (1 << sci_mode::SM_SDINEW) | (1 << sci_mode::SM_RESET))
    }

    /// Stop/cancel playing current
    pub fn stop_play(&mut self) -> Result<(), VS1053Error<BUS::Error, CS::Error, DC::Error>> {
        // see section 10.5.2 of VLSI datasheet - canceling playback
        // if space ever becomes and issue dont use a fixed buffer
        let end_fill_byte_buf = [self.end_fill_byte; END_FILL_BYTE_BUF_LEN as usize];

        self.write_cancel()?;

        // manual says If SM_CANCEL doesn’t clear after 2048 bytes
        for _ in 0..64 {
            // send silence of 32b
            let buf = [self.end_fill_byte; VS1053_CHUNK_SIZE as usize];
            self.write(&buf)?;

            if let Ok(cancel_value) = self.read_cancel() {
                if cancel_value == 0 {
                    self.write(&end_fill_byte_buf)?;
                    return Ok(());
                }
            }
            self.delay.delay_ms(10);
        }
        // manual says to do a software reset but I did not see a problem so far
        // leave it to the user to call soft_reset on its own
        // just to mention that stuff like set_mp3_mode needs to be called
        // again after a soft reset
        Err(VS1053Error::StopError)
    }

    /// Test communication with chip
    pub fn get_chip_connected(&mut self) -> Result<bool, VS1053Error<BUS::Error, CS::Error, DC::Error>> {
        let status = self.read_register(regs::SCI_STATUS)?;

        Ok(!(status == 0 || status == 0xffff))
    }

    /// Get the Version Number for the VLSI chip
    /// from VLSI datasheet: 0 for VS1001, 1 for VS1011, 2 for VS1002, 3 for VS1003, 4 for VS1053 and VS8053,
    /// 5 for VS1033, 7 for VS1103, and 6 for VS1063.
    pub fn get_chip_version(&mut self) -> Result<u16, VS1053Error<BUS::Error, CS::Error, DC::Error>> {
        let status = self.read_register(regs::SCI_STATUS)?;

        Ok((status & 0x00f0) >> 4)
    }

    fn wram_write(&mut self, address: u16, data: u16) -> Result<(), VS1053Error<BUS::Error, CS::Error, DC::Error>> {
        self.write_register(regs::SCI_WRAMADDR, address)?;
        self.write_register(regs::SCI_WRAM, data)?;
        Ok(())
    }

    fn wram_read(&mut self, address: u16) -> Result<u16, VS1053Error<BUS::Error, CS::Error, DC::Error>> {
        self.write_register(regs::SCI_WRAMADDR, address)?;
        let value = self.read_register(regs::SCI_WRAM)?;
        Ok(value)
    }

    /// Depends on board defaults and can be optional
    /// Calls soft_reset
    pub fn set_mp3_mode_on(&mut self) -> Result<(), VS1053Error<BUS::Error, CS::Error, DC::Error>> {
        self.wram_write(ADDR_REG_GPIO_DDR_RW, 3)?; // GPIO DDR = 3
        self.wram_write(ADDR_REG_GPIO_ODATA_RW, 0)?; // GPIO ODATA = 0
        // TODO delay needed
        self.delay.delay_ms(100);
        self.soft_reset()?;
        Ok(())
    }

    pub fn set_stream_mode_on(&mut self) -> Result<(), VS1053Error<BUS::Error, CS::Error, DC::Error>> {
        self.write_register(
            regs::SCI_MODE,
            (1 << sci_mode::SM_SDINEW) | (1 << sci_mode::SM_STREAM)
        )?;
        // TODO delay needed
        self.delay.delay_ms(10);
        self.await_data_request();
        Ok(())
    }

    pub fn set_stream_mode_off(&mut self) -> Result<(), VS1053Error<BUS::Error, CS::Error, DC::Error>> {
        self.write_register(regs::SCI_MODE, 1 << sci_mode::SM_SDINEW)?;
        // TODO delay needed
        self.delay.delay_ms(10);
        self.await_data_request();
        Ok(())
    }

    /// see section 9.6.5 of VLSI datasheet
    pub fn get_decode_time(&mut self) -> Result<u16, VS1053Error<BUS::Error, CS::Error, DC::Error>> {
        let seconds = self.read_register(regs::SCI_DECODE_TIME)?;
        Ok(seconds)
    }

    /// see section 9.6.5 of VLSI datasheet
    pub fn clear_decode_time(&mut self) -> Result<(), VS1053Error<BUS::Error, CS::Error, DC::Error>> {
        self.write_register(regs::SCI_DECODE_TIME, 0u16)?;
        self.write_register(regs::SCI_DECODE_TIME, 0u16)
    }

    /// see section 9.6.9 of VLSI datasheet
    /// get current value of SCI_HDAT0 and SCI_HDAT1
    pub fn get_decode_details(&mut self) -> Result<(u16, u16), VS1053Error<BUS::Error, CS::Error, DC::Error>> {
        let hdat0 = self.read_register(regs::SCI_HDAT0)?;
        let hdat1 = self.read_register(regs::SCI_HDAT1)?;

        Ok((hdat0, hdat1))
    }

    fn load_user_code(&mut self, patches: &[u16]) -> Result<(), VS1053Error<BUS::Error, CS::Error, DC::Error>> {
        let mut idx = 0;
        while idx < patches.len() {
            let addr = patches[idx].to_be_bytes();
            idx += 1;
            let mut n = patches[idx];
            idx += 1;
            if (n & 0x8000) != 0 {
                /* RLE run, replicate n samples */
                n &= 0x7fff;
                let val = patches[idx];
                idx += 1;
                for _ in 0..n {
                    self.write_register(addr[1], val)?;
                }
            } else {
                /* Copy run, copy n samples */
                for _ in 0..n {
                    let val = patches[idx];
                    idx += 1;
                    self.write_register(addr[1], val)?;
                }
            }
        }
        Ok(())
    }

    /// Should only be called if chip version is 4
    pub fn load_default_patches(&mut self) -> Result<(), VS1053Error<BUS::Error, CS::Error, DC::Error>> {
        self.load_user_code(&vs1053b_patches::PATCHES)
    }

    pub fn load_flac_patches(&mut self) -> Result<(), VS1053Error<BUS::Error, CS::Error, DC::Error>> {
        self.load_user_code(&vs1053b_patches_flac::PATCHES_FLAC)
    }
}