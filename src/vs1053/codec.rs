use bitfields::bitfield;

/// see section 9.6.9 of VLSI datasheet
/// contents of SCI_HDAT1 register depending on codec
///
/// usage: to get codec type currently decoding
/// will take some time until data is available after starting a stream
/// ```
/// ...
/// if let Ok((hdat0_value, hdat1_value)) = vs1053.get_decode_details() {
///     let codec = Codec::try_from(hdat1_value).unwrap_or(Codec::UnknownType);
/// }
/// ...
/// ```
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum Codec {
    WAVType,
    AACType,
    WMAType,
    MIDIType,
    OGGType,
    M4AType,
    FLACType,
    MP3Type,
    UnknownType,
}

impl From<u16> for Codec {
    fn from(value: u16) -> Self {
        match value {
            SCI_HDAT1_WAV => Codec::WAVType,
            SCI_HDAT1_AAC => Codec::AACType,
            SCI_HDAT1_WMA => Codec::WMAType,
            SCI_HDAT1_MIDI => Codec::MIDIType,
            SCI_HDAT1_OGG => Codec::OGGType,
            SCI_HDAT1_M4A => Codec::M4AType,
            SCI_HDAT1_FLAC => Codec::FLACType,
            d if d >= SCI_HDAT1_MP3_MIN && d <= SCI_HDAT1_MP3_MAX => Codec::MP3Type,
            _ => Codec::UnknownType,
        }
    }
}

/// for all except mp3 SCI_HDAT0 contains the data rate measured
/// in bytes per second To get the bitrate, multiply the value by 8
pub const SCI_HDAT1_WAV: u16 = 0x7665;
pub const SCI_HDAT1_AAC: u16 = 0x4154;
pub const SCI_HDAT1_WMA: u16 = 0x574D;
pub const SCI_HDAT1_MIDI: u16 = 0x4D54;
pub const SCI_HDAT1_OGG: u16 = 0x4F67;
pub const SCI_HDAT1_M4A: u16 = 0x4D34;
pub const SCI_HDAT1_FLAC: u16 = 0x664C;

/// this is a min value - all >= is mp3
pub const SCI_HDAT1_MP3_MIN: u16 = 0xFFE0;
pub const SCI_HDAT1_MP3_MAX: u16 = 0xFFFF;

/// mp3 specific SCI_HDAT0 and SCI_HDAT1 parsing
/// optional enabled by using bitfield_struct feature
///
/// usage:
/// ```
///  ...
///  if let Ok((hdat0_value, hdat1_value)) = vs1053.get_decode_details() {
///     let codec = Codec::try_from(hdat1_value).unwrap_or(Codec::UnknownType);
///     if codec == Codec::MP3Type {
///         let mp3_type = MP3CodecType::from_bits(hdat1_value);
///         let mp3_data = MP3CodecData::from_bits(hdat0_value);
///         let bitrate = map_mp3_bitrate(mp3_type.layer(), mp3_type.id(), mp3_data.bit_rate());
///     }
///  }
/// ...
/// ```

#[bitfield(u16)]
#[derive(Copy, Clone)]
pub struct MP3CodecType {
    #[bits(1)]
    protect_bit: bool,
    #[bits(2)]
    layer: u8,
    #[bits(2)]
    id: u8,
    #[bits(11)]
    sync_word: u16,
}

#[bitfield(u16)]
#[derive(Copy, Clone)]
pub struct MP3CodecData {
    #[bits(2)]
    emphasis: u8,
    #[bits(1)]
    original: bool,
    #[bits(1)]
    copyright: bool,
    #[bits(2)]
    extension: u8,
    #[bits(2)]
    mode: u8,
    #[bits(1)]
    private: bool,
    #[bits(1)]
    pad: bool,
    #[bits(2)]
    sample_rate: u8,
    #[bits(4)]
    bit_rate: u8,
}

/// see section 9.6.5 of VLSI datasheet
pub const MP3_BITRATE_MAPPING: [[u16; 6]; 16] = [
    [0, 0, 0, 0, 0, 0, ],
    [32, 32, 32, 8, 32, 8, ],
    [64, 48, 48, 16, 40, 16],
    [96, 56, 56, 24, 48, 24],
    [128, 64, 64, 32, 56, 32],
    [160, 80, 80, 40, 64, 40],
    [192, 96, 96, 48, 80, 48],
    [224, 112, 112, 56, 96, 56],
    [256, 128, 128, 64, 112, 64],
    [288, 144, 160, 80, 128, 80],
    [320, 160, 192, 96, 160, 96],
    [352, 176, 224, 112, 192, 112],
    [384, 192, 256, 128, 224, 128],
    [416, 224, 320, 144, 256, 144],
    [448, 256, 384, 160, 320, 160,],
    [0, 0, 0, 0, 0, 0, ],
];

pub fn map_mp3_bitrate(layer: u8, id: u8, bitrate: u8) -> u16 {
    let layer_idx = match layer {
        3 => 0,
        2 => 2,
        1 => 4,
        _ => 0
    };
    let idx_idx = match id {
        3 => 0,
        _ => 1,
    };
    let table_x = (layer_idx + idx_idx) as usize;
    if bitrate <= 15 && table_x <= 5 {
        return MP3_BITRATE_MAPPING[bitrate as usize][table_x]
    }
    0
}