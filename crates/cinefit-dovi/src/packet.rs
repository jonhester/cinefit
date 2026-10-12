//! Packets: from parity bits to the metadata structure's bytes (ETSI GS CCM
//! 001 clauses 6.3 and 6.4).

use std::fmt;

use crate::metadata::{self, DmMetadata};

pub const PACKET_BYTES: usize = 128;
/// One copy of a packet: one bit per pixel.
const COPY_PIXELS: usize = PACKET_BYTES * 8;
/// Every packet is sent three times in a row (clause 6.4.3).
const COPIES: usize = 3;
const PACKET_PIXELS: usize = COPY_PIXELS * COPIES;

const HEADER: usize = 3;
const BODY: usize = 121;
/// A single or first packet spends two body bytes on the length.
const FIRST_PAYLOAD: usize = BODY - 2;
/// The largest structure clause 6.3.3.2 allows.
const MAX_LENGTH: usize = 0x2F00;

static CRC: crc::Crc<u32> = crc::Crc::<u32>::new(&crc::CRC_32_MPEG_2);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacketType {
    /// The whole structure in one packet.
    Single,
    First,
    Middle,
    Last,
}

/// A packet header (clause 6.3.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub packet_type: PacketType,
    pub metadata_type: u8,
    pub affected_metadata_id: u8,
    pub metadata_version: u8,
    pub current_metadata_id: u8,
    /// No metadata in this picture: the sink ignores the body.
    pub no_metadata: bool,
    /// The next picture is not HDR: the sink switches to SDR.
    pub end_of_stream: bool,
}

impl Header {
    fn parse(b: &[u8; PACKET_BYTES]) -> Self {
        Self {
            packet_type: match b[0] >> 6 {
                0 => PacketType::Single,
                1 => PacketType::First,
                2 => PacketType::Middle,
                _ => PacketType::Last,
            },
            metadata_type: (b[0] >> 4) & 0b11,
            affected_metadata_id: b[0] & 0x0F,
            metadata_version: b[1] >> 5,
            current_metadata_id: (b[1] >> 1) & 0x0F,
            no_metadata: b[1] & 1 != 0,
            end_of_stream: b[2] & 1 != 0,
        }
    }
}

/// One frame's tunnel, decoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tunnel {
    /// The first packet's header.
    pub header: Header,
    /// For each packet, which of its three copies passed the CRC (0-2).
    /// Anything but 0 means the frame lost bits on the way.
    pub copies_used: Vec<u8>,
    /// `None` when the header says the picture carries no metadata.
    pub metadata: Option<DmMetadata>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// No copy of packet `packet` passed its CRC. For packet 0 this is what a
    /// frame without the tunnel looks like.
    NoPacket { packet: usize },
    /// The packets passed their CRCs but do not form a valid structure.
    Malformed(&'static str),
    /// The structure decoded but its contents do not parse.
    Metadata(metadata::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NoPacket { packet } => write!(f, "no copy of packet {packet} passed its CRC"),
            Error::Malformed(why) => write!(f, "malformed packets: {why}"),
            Error::Metadata(e) => write!(f, "metadata: {e}"),
        }
    }
}

impl std::error::Error for Error {}

/// Packet `p`, from the first of its three copies whose CRC passes.
fn packet(
    parity: &impl Fn(usize) -> Option<bool>,
    p: usize,
) -> Result<([u8; PACKET_BYTES], u8), Error> {
    'copies: for copy in 0..COPIES {
        let start = p * PACKET_PIXELS + copy * COPY_PIXELS;
        let mut bytes = [0u8; PACKET_BYTES];
        // The first byte first, most significant bit first (clause 6.4.1).
        for (n, byte) in bytes.iter_mut().enumerate() {
            for m in 0..8 {
                let Some(bit) = parity(start + n * 8 + m) else {
                    break 'copies;
                };
                *byte = *byte << 1 | u8::from(bit);
            }
        }
        // Over the whole packet, CRC included, a good packet checks to zero.
        if CRC.checksum(&bytes) == 0 {
            return Ok((bytes, copy as u8));
        }
    }
    Err(Error::NoPacket { packet: p })
}

pub(crate) fn decode(parity: &impl Fn(usize) -> Option<bool>) -> Result<Tunnel, Error> {
    let (first, copy) = packet(parity, 0)?;
    let header = Header::parse(&first);
    let mut copies_used = vec![copy];
    if header.no_metadata {
        return Ok(Tunnel {
            header,
            copies_used,
            metadata: None,
        });
    }

    let body = &first[HEADER..HEADER + BODY];
    let length = usize::from(body[0]) << 8 | usize::from(body[1]);
    let mut structure = Vec::with_capacity(length);
    match header.packet_type {
        PacketType::Single => {
            if length > FIRST_PAYLOAD {
                return Err(Error::Malformed("single packet longer than 119 bytes"));
            }
            structure.extend_from_slice(&body[2..2 + length]);
        }
        PacketType::First => {
            if length <= FIRST_PAYLOAD || length > MAX_LENGTH {
                return Err(Error::Malformed("first packet with an impossible length"));
            }
            structure.extend_from_slice(&body[2..]);
            let mut p = 1;
            while structure.len() < length {
                let (bytes, copy) = packet(parity, p)?;
                copies_used.push(copy);
                let h = Header::parse(&bytes);
                let remaining = length - structure.len();
                let expected = if remaining > BODY {
                    PacketType::Middle
                } else {
                    PacketType::Last
                };
                if h.packet_type != expected {
                    return Err(Error::Malformed("packets out of sequence"));
                }
                if h.affected_metadata_id != header.affected_metadata_id {
                    return Err(Error::Malformed("packets for different pictures"));
                }
                let take = remaining.min(BODY);
                structure.extend_from_slice(&bytes[HEADER..HEADER + take]);
                p += 1;
            }
        }
        PacketType::Middle | PacketType::Last => {
            return Err(Error::Malformed("first packet is a continuation"));
        }
    }

    let metadata = DmMetadata::parse(&structure).map_err(Error::Metadata)?;
    Ok(Tunnel {
        header,
        copies_used,
        metadata: Some(metadata),
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Splits a structure into packets as a source does (clause 6.3), each
    /// with its CRC.
    pub(crate) fn packetize(structure: &[u8], affected_id: u8) -> Vec<[u8; PACKET_BYTES]> {
        let mut packets = Vec::new();
        let header = |kind: u8| [kind << 6 | affected_id, affected_id << 1, 0];
        let mut push = |kind: u8, body: &[u8]| {
            let mut p = [0u8; PACKET_BYTES];
            p[..HEADER].copy_from_slice(&header(kind));
            p[HEADER..HEADER + body.len()].copy_from_slice(body);
            let crc = CRC.checksum(&p[..PACKET_BYTES - 4]);
            p[PACKET_BYTES - 4..].copy_from_slice(&crc.to_be_bytes());
            packets.push(p);
        };
        let len = (structure.len() as u16).to_be_bytes();
        if structure.len() <= FIRST_PAYLOAD {
            push(0, &[&len[..], structure].concat());
        } else {
            push(1, &[&len[..], &structure[..FIRST_PAYLOAD]].concat());
            let rest: Vec<_> = structure[FIRST_PAYLOAD..].chunks(BODY).collect();
            for (k, chunk) in rest.iter().enumerate() {
                push(if k + 1 == rest.len() { 3 } else { 2 }, chunk);
            }
        }
        packets
    }

    /// The bits a frame carries: every packet three times, MSB first.
    pub(crate) fn bits(packets: &[[u8; PACKET_BYTES]]) -> Vec<bool> {
        let mut out = Vec::new();
        for p in packets {
            for _ in 0..COPIES {
                for byte in p {
                    out.extend((0..8).rev().map(|m| byte >> m & 1 == 1));
                }
            }
        }
        out
    }

    fn decode_bits(bits: &[bool]) -> Result<Tunnel, Error> {
        decode(&|i| bits.get(i).copied())
    }

    #[test]
    fn crc_matches_the_mpeg2_check_value() {
        // The standard check: CRC-32/MPEG-2 of "123456789" is 0x0376E6E7.
        assert_eq!(CRC.checksum(b"123456789"), 0x0376_E6E7);
    }

    #[test]
    fn a_frame_without_the_tunnel_has_no_packet() {
        let bits = vec![false; PACKET_PIXELS];
        assert_eq!(
            decode_bits(&bits).err(),
            Some(Error::NoPacket { packet: 0 })
        );
    }

    #[test]
    fn a_damaged_first_copy_falls_back_to_the_second() {
        let s = metadata::tests::structure(&[]);
        let mut bits = bits(&packetize(&s, 0));
        bits[5] = !bits[5];
        let t = decode_bits(&bits).expect("second copy");
        assert_eq!(t.copies_used, [1]);
    }

    #[test]
    fn all_three_copies_damaged_fails() {
        let s = metadata::tests::structure(&[]);
        let mut bits = bits(&packetize(&s, 0));
        for copy in 0..COPIES {
            bits[copy * COPY_PIXELS + 9] ^= true;
        }
        assert_eq!(
            decode_bits(&bits).err(),
            Some(Error::NoPacket { packet: 0 })
        );
    }

    #[test]
    fn a_long_structure_spans_packets() {
        // 148 bytes, as seen from a UB820: a first packet and a last.
        let s = metadata::tests::structure(&metadata::tests::UB820_BLOCKS);
        assert_eq!(s.len(), 148);
        let packets = packetize(&s, 5);
        assert_eq!(packets.len(), 2);
        let t = decode_bits(&bits(&packets)).expect("decodes");
        assert_eq!(t.header.packet_type, PacketType::First);
        assert_eq!(t.header.affected_metadata_id, 5);
        assert_eq!(t.header.current_metadata_id, 5);
        assert_eq!(t.copies_used, [0, 0]);
        assert_eq!(t.metadata.expect("metadata").level2.len(), 3);
    }

    #[test]
    fn a_missing_second_packet_is_reported_as_such() {
        let s = metadata::tests::structure(&metadata::tests::UB820_BLOCKS);
        let packets = packetize(&s, 0);
        let bits = bits(&packets[..1]);
        assert_eq!(
            decode_bits(&bits).err(),
            Some(Error::NoPacket { packet: 1 })
        );
    }

    #[test]
    fn no_metadata_flag_skips_the_body() {
        let s = metadata::tests::structure(&[]);
        let mut packets = packetize(&s, 0);
        packets[0][1] |= 1;
        let crc = CRC.checksum(&packets[0][..PACKET_BYTES - 4]);
        packets[0][PACKET_BYTES - 4..].copy_from_slice(&crc.to_be_bytes());
        let t = decode_bits(&bits(&packets)).expect("decodes");
        assert!(t.header.no_metadata);
        assert!(t.metadata.is_none());
    }
}
