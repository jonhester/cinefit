//! The few things `cinefit display` reads from a display's EDID: who made it,
//! what it calls itself, and what its CTA extension says about HDR and wide
//! colour. Pure parsing, so it is tested anywhere.

/// What an EDID says, as far as `cinefit display` reports it.
#[derive(Debug, Default, PartialEq)]
pub struct Summary {
    /// The three-letter manufacturer ID (`JKC` for JVC Kenwood).
    pub manufacturer: String,
    /// The monitor name descriptor, if there is one.
    pub name: Option<String>,
    /// The HDR static metadata block's transfer functions, by name; `None`
    /// when the EDID has no such block (no HDR at all).
    pub eotfs: Option<Vec<&'static str>>,
    /// The colorimetry block's flags, by name; empty when there is none.
    pub colorimetry: Vec<&'static str>,
}

/// Reads `edid`. `None` if it is too short or lacks the EDID header.
pub fn summary(edid: &[u8]) -> Option<Summary> {
    const HEADER: [u8; 8] = [0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x00];
    if edid.len() < 128 || edid[..8] != HEADER {
        return None;
    }
    let id = u16::from_be_bytes([edid[8], edid[9]]);
    let letter = |shift: u16| char::from(b'@' + ((id >> shift) & 0x1f) as u8);
    let manufacturer = [letter(10), letter(5), letter(0)].iter().collect();
    // Four 18-byte descriptors; tag 0xfc is the monitor name.
    let name = (54..126).step_by(18).find_map(|d| {
        (edid[d..d + 3] == [0, 0, 0] && edid[d + 3] == 0xfc).then(|| {
            String::from_utf8_lossy(&edid[d + 5..d + 18])
                .trim_end_matches(['\n', ' '])
                .to_string()
        })
    });
    let mut s = Summary {
        manufacturer,
        name,
        ..Summary::default()
    };
    // CTA-861 extensions: data blocks from byte 4 to the DTD offset.
    for ext in edid[128..]
        .as_chunks::<128>()
        .0
        .iter()
        .filter(|b| b[0] == 0x02)
    {
        let end = usize::from(ext[2]).clamp(4, 127);
        let mut i = 4;
        while i < end {
            let tag = ext[i] >> 5;
            let len = usize::from(ext[i] & 0x1f);
            let block = &ext[(i + 1).min(end)..(i + 1 + len).min(end)];
            // Tag 7 is "extended": its first byte says which.
            if tag == 7 && !block.is_empty() {
                match block[0] {
                    0x05 if block.len() >= 2 => {
                        const NAMES: [&str; 8] = [
                            "xvYCC601",
                            "xvYCC709",
                            "sYCC601",
                            "opYCC601",
                            "opRGB",
                            "BT2020cYCC",
                            "BT2020YCC",
                            "BT2020RGB",
                        ];
                        s.colorimetry.extend(
                            (0..8)
                                .filter(|b| block[1] & (1 << b) != 0)
                                .map(|b| NAMES[b]),
                        );
                    }
                    0x06 if block.len() >= 2 => {
                        const NAMES: [&str; 4] = ["SDR", "HDR gamma", "PQ (ST 2084)", "HLG"];
                        s.eotfs = Some(
                            (0..4)
                                .filter(|b| block[1] & (1 << b) != 0)
                                .map(|b| NAMES[b])
                                .collect(),
                        );
                    }
                    _ => {}
                }
            }
            i += 1 + len;
        }
    }
    Some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A base block plus one CTA extension carrying `blocks`.
    fn edid(id: [u8; 2], name: &str, blocks: &[&[u8]]) -> Vec<u8> {
        let mut e = vec![0u8; 256];
        e[..8].copy_from_slice(&[0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x00]);
        e[8..10].copy_from_slice(&id);
        e[54..59].copy_from_slice(&[0, 0, 0, 0xfc, 0]);
        let mut n = name.as_bytes().to_vec();
        n.push(b'\n');
        n.resize(13, b' ');
        e[59..72].copy_from_slice(&n);
        e[128] = 0x02;
        e[129] = 0x03;
        let mut at = 132;
        for b in blocks {
            e[at..at + b.len()].copy_from_slice(b);
            at += b.len();
        }
        e[130] = at as u8 - 128;
        e
    }

    #[test]
    fn a_projector_with_hdr_and_bt2020() {
        // JKC: J=10, K=11, C=3 -> 0b0_01010_01011_00011.
        let e = edid(
            [0x29, 0x63],
            "ILAFPJ--X184A",
            &[
                &[0xe3, 0x05, 0xc0, 0x00], // colorimetry: BT2020YCC, BT2020RGB
                &[0xe3, 0x06, 0x0f, 0x01], // HDR: all four EOTFs, type 1
                &[0x23, 0x09, 0x07, 0x07], // an audio block, skipped
            ],
        );
        let s = summary(&e).unwrap();
        assert_eq!(s.manufacturer, "JKC");
        assert_eq!(s.name.as_deref(), Some("ILAFPJ--X184A"));
        assert_eq!(
            s.eotfs,
            Some(vec!["SDR", "HDR gamma", "PQ (ST 2084)", "HLG"])
        );
        assert_eq!(s.colorimetry, ["BT2020YCC", "BT2020RGB"]);
    }

    #[test]
    fn no_hdr_block_is_no_hdr() {
        let e = edid([0x29, 0x63], "PLAIN", &[&[0x23, 0x09, 0x07, 0x07]]);
        let s = summary(&e).unwrap();
        assert_eq!(s.eotfs, None);
        assert!(s.colorimetry.is_empty());
    }

    #[test]
    fn not_an_edid() {
        assert_eq!(summary(&[0u8; 128]), None);
        assert_eq!(summary(&[0u8; 10]), None);
    }
}
