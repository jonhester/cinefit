//! The metadata structure itself, `dm_metadata()` (ETSI GS CCM 001 clause
//! 6.2).
//!
//! A disc's RPU carries the same information packed into variable-width bits,
//! which libdovi (the parser behind `dovi_tool`,
//! <https://github.com/quietvoid/dovi_tool>) reads. The HDMI form is
//! byte-aligned and simpler, so it is parsed here instead. The block types
//! use libdovi's field names and units, so decoded values compare directly
//! with `dovi_tool export` and `dovi_tool plot` for a rip of the same disc.

use std::fmt;

/// Level 1: the shot's brightness, as 12-bit PQ codes. libdovi's
/// `ExtMetadataBlockLevel1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Level1 {
    pub min_pq: u16,
    pub max_pq: u16,
    pub avg_pq: u16,
}

/// Level 2: the colourist's trim for one target display. The trims are 12-bit
/// with 2048 neutral. libdovi's `ExtMetadataBlockLevel2`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Level2 {
    pub target_max_pq: u16,
    pub trim_slope: u16,
    pub trim_offset: u16,
    pub trim_power: u16,
    pub trim_chroma_weight: u16,
    pub trim_saturation_gain: u16,
    pub ms_weight: i16,
}

/// Level 4. Not in the ETSI table, but sent by a UB820 on every frame.
/// libdovi's `ExtMetadataBlockLevel4`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Level4 {
    pub anchor_pq: u16,
    pub anchor_power: u16,
}

/// Level 5: the active picture area, as offsets in pixels. libdovi's
/// `ExtMetadataBlockLevel5`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Level5 {
    pub active_area_left_offset: u16,
    pub active_area_right_offset: u16,
    pub active_area_top_offset: u16,
    pub active_area_bottom_offset: u16,
}

/// Bytes before the first extension block.
const FIXED: usize = 71;
/// An extension block's length (4 bytes) and level (1 byte).
const BLOCK_HEADER: usize = 5;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DmMetadata {
    pub scene_refresh: bool,
    /// The source's YCbCr to RGB matrix and offsets, as sent (libplacebo and
    /// libdovi divide the coefficients by 8192).
    pub ycc_to_rgb: [i16; 9],
    pub ycc_to_rgb_offset: [u32; 3],
    /// RGB to LMS, as sent.
    pub rgb_to_lms: [i16; 9],
    pub signal_bit_depth: u8,
    /// 2 means IPT, which is what tunnels carry.
    pub signal_color_space: u8,
    /// The grade's range, as 12-bit PQ codes (0-4095).
    pub source_min_pq: u16,
    pub source_max_pq: u16,
    /// Per-shot brightness.
    pub level1: Option<Level1>,
    /// The colourist's trims, one per target display.
    pub level2: Vec<Level2>,
    pub level4: Option<Level4>,
    /// The active area. Defined for HDMI; a UB820 has not sent it, but only
    /// content without bars has been captured, where there is none to send.
    pub level5: Option<Level5>,
    /// Blocks of any other level, kept as raw `(level, payload)`.
    pub other: Vec<(u8, Vec<u8>)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// The structure ends before its fixed fields do.
    TooShort,
    /// An extension block runs past the end of the structure.
    BlockOverrun { level: u8 },
    /// A block of a known level has the wrong payload size.
    BlockSize { level: u8, length: usize },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::TooShort => f.write_str("structure shorter than its fixed fields"),
            Error::BlockOverrun { level } => {
                write!(f, "level {level} block runs past the structure")
            }
            Error::BlockSize { level, length } => {
                write!(f, "level {level} block of {length} bytes")
            }
        }
    }
}

impl std::error::Error for Error {}

fn u16_at(b: &[u8], o: usize) -> u16 {
    u16::from_be_bytes([b[o], b[o + 1]])
}

fn i16_at(b: &[u8], o: usize) -> i16 {
    i16::from_be_bytes([b[o], b[o + 1]])
}

impl DmMetadata {
    /// Parses a reassembled `dm_metadata()` structure.
    ///
    /// The reserved bytes are not checked: a source that sets them differently
    /// is still worth reading, and the CRC already vouches for the bytes.
    ///
    /// # Errors
    ///
    /// If the structure is shorter than its fixed part, or an extension block
    /// does not fit or has the wrong size for its level.
    pub fn parse(b: &[u8]) -> Result<Self, Error> {
        if b.len() < FIXED {
            return Err(Error::TooShort);
        }
        let mut m = Self {
            scene_refresh: b[1] != 0,
            ycc_to_rgb: std::array::from_fn(|j| i16_at(b, 2 + 2 * j)),
            ycc_to_rgb_offset: std::array::from_fn(|j| {
                u32::from_be_bytes(b[20 + 4 * j..24 + 4 * j].try_into().expect("4 bytes"))
            }),
            rgb_to_lms: std::array::from_fn(|j| i16_at(b, 32 + 2 * j)),
            signal_bit_depth: b[60],
            signal_color_space: b[61],
            source_min_pq: u16_at(b, 64),
            source_max_pq: u16_at(b, 66),
            level1: None,
            level2: Vec::new(),
            level4: None,
            level5: None,
            other: Vec::new(),
        };

        let mut o = FIXED;
        for _ in 0..b[70] {
            let header = b.get(o..o + BLOCK_HEADER).ok_or(Error::TooShort)?;
            let length = u32::from_be_bytes(header[..4].try_into().expect("4 bytes")) as usize;
            let level = header[4];
            o += BLOCK_HEADER;
            let p = b
                .get(o..o.saturating_add(length))
                .ok_or(Error::BlockOverrun { level })?;
            o += length;
            let size = |want: usize| {
                if length == want {
                    Ok(())
                } else {
                    Err(Error::BlockSize { level, length })
                }
            };
            match level {
                1 => {
                    size(6)?;
                    m.level1 = Some(Level1 {
                        min_pq: u16_at(p, 0),
                        max_pq: u16_at(p, 2),
                        avg_pq: u16_at(p, 4),
                    });
                }
                2 => {
                    size(14)?;
                    m.level2.push(Level2 {
                        target_max_pq: u16_at(p, 0),
                        trim_slope: u16_at(p, 2),
                        trim_offset: u16_at(p, 4),
                        trim_power: u16_at(p, 6),
                        trim_chroma_weight: u16_at(p, 8),
                        trim_saturation_gain: u16_at(p, 10),
                        ms_weight: i16_at(p, 12),
                    });
                }
                4 => {
                    size(4)?;
                    m.level4 = Some(Level4 {
                        anchor_pq: u16_at(p, 0),
                        anchor_power: u16_at(p, 2),
                    });
                }
                5 => {
                    size(8)?;
                    m.level5 = Some(Level5 {
                        active_area_left_offset: u16_at(p, 0),
                        active_area_right_offset: u16_at(p, 2),
                        active_area_top_offset: u16_at(p, 4),
                        active_area_bottom_offset: u16_at(p, 6),
                    });
                }
                _ => m.other.push((level, p.to_vec())),
            }
        }
        Ok(m)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// The blocks a UB820 was seen sending: L1, three L2 trims, L4.
    pub(crate) const UB820_BLOCKS: [(u8, &[u8]); 5] = [
        (1, &[0x00, 62, 0x0E, 0x66, 0x03, 0x33]),
        (
            2,
            &[
                0x08, 0x21, 0x08, 0x3D, 0x08, 0x00, 0x09, 0xBB, 0x03, 0xF8, 0x08, 0x7B, 0x02, 0x00,
            ],
        ),
        (
            2,
            &[
                0x0B, 0x23, 0x06, 0x3A, 0x07, 0x8F, 0x05, 0xEB, 0x09, 0xAA, 0x07, 0xF4, 0x0F, 0xFF,
            ],
        ),
        (
            2,
            &[
                0x0C, 0x07, 0x05, 0x85, 0x07, 0x83, 0x05, 0x49, 0x0A, 0xA0, 0x09, 0x04, 0x0F, 0xFF,
            ],
        ),
        (4, &[0x01, 0xDD, 0x02, 0xDC]),
    ];

    /// A `dm_metadata()` structure with the given extension blocks.
    pub(crate) fn structure(blocks: &[(u8, &[u8])]) -> Vec<u8> {
        let mut s = vec![0x00, 0x00];
        for c in [8192i16, 799, 1681, 8192, -933, 1091, 8192, 267, -5545] {
            s.extend(c.to_be_bytes());
        }
        // As a UB820 sends them: luma 0, chroma centred (over 2^28).
        for off in [0u32, 0x0800_0000, 0x0800_0000] {
            s.extend(off.to_be_bytes());
        }
        for c in [17081i16, -349, -349, -349, 17081, -349, -349, -349, 17081] {
            s.extend(c.to_be_bytes());
        }
        s.extend([0xFF, 0xFF]);
        s.extend([0u8; 8]);
        s.extend([12, 2, 0x01, 0x01]);
        s.extend(62u16.to_be_bytes());
        s.extend(3696u16.to_be_bytes());
        s.extend([0x00, 0x2A, blocks.len() as u8]);
        assert_eq!(s.len(), FIXED);
        for (level, payload) in blocks {
            s.extend((payload.len() as u32).to_be_bytes());
            s.push(*level);
            s.extend(*payload);
        }
        s
    }

    #[test]
    fn fixed_fields_parse() {
        let m = DmMetadata::parse(&structure(&[])).expect("parses");
        assert_eq!(m.ycc_to_rgb[0], 8192);
        assert_eq!(m.ycc_to_rgb[4], -933);
        assert_eq!(m.rgb_to_lms[1], -349);
        assert_eq!(m.signal_bit_depth, 12);
        assert_eq!(m.signal_color_space, 2);
        assert_eq!((m.source_min_pq, m.source_max_pq), (62, 3696));
        assert!(!m.scene_refresh);
    }

    #[test]
    fn the_blocks_a_ub820_sends_parse() {
        let m = DmMetadata::parse(&structure(&UB820_BLOCKS)).expect("parses");
        let l1 = m.level1.expect("L1");
        assert_eq!((l1.min_pq, l1.max_pq, l1.avg_pq), (62, 3686, 819));
        let targets: Vec<_> = m.level2.iter().map(|b| b.target_max_pq).collect();
        assert_eq!(targets, [2081, 2851, 3079]); // 100, 600 and 1000 nits
        assert_eq!(m.level2[0].ms_weight, 512);
        let l4 = m.level4.expect("L4");
        assert_eq!((l4.anchor_pq, l4.anchor_power), (477, 732));
        assert!(m.level5.is_none());
        assert!(m.other.is_empty());
    }

    #[test]
    fn unknown_levels_are_kept_raw() {
        let m = DmMetadata::parse(&structure(&[(9, &[1, 2, 3])])).expect("parses");
        assert_eq!(m.other, [(9, vec![1, 2, 3])]);
    }

    #[test]
    fn a_block_past_the_end_is_an_error() {
        let mut s = structure(&[(1, &[0; 6])]);
        s.truncate(s.len() - 1);
        assert_eq!(
            DmMetadata::parse(&s).err(),
            Some(Error::BlockOverrun { level: 1 })
        );
    }

    #[test]
    fn a_known_level_of_the_wrong_size_is_an_error() {
        let s = structure(&[(1, &[0; 4])]);
        assert_eq!(
            DmMetadata::parse(&s).err(),
            Some(Error::BlockSize {
                level: 1,
                length: 4
            })
        );
    }

    #[test]
    fn a_short_structure_is_an_error() {
        assert_eq!(DmMetadata::parse(&[0; 10]).err(), Some(Error::TooShort));
    }
}
