//! TLV8, the pairing messages' encoding: a tag byte, a length byte, the
//! value. A value longer than 255 bytes is split across repeats of its tag.
//!
//! As pyatv reads and writes it (MIT; `pyatv/auth/hap_tlv8.py`), and the
//! tag values are its.

use std::collections::HashMap;

pub const METHOD: u8 = 0x00;
pub const IDENTIFIER: u8 = 0x01;
pub const SALT: u8 = 0x02;
pub const PUBLIC_KEY: u8 = 0x03;
pub const PROOF: u8 = 0x04;
pub const ENCRYPTED_DATA: u8 = 0x05;
pub const SEQ_NO: u8 = 0x06;
pub const ERROR: u8 = 0x07;
pub const SIGNATURE: u8 = 0x0a;

pub fn write(items: &[(u8, &[u8])]) -> Vec<u8> {
    let mut out = Vec::new();
    for &(tag, value) in items {
        for chunk in value.chunks(255) {
            out.push(tag);
            out.push(chunk.len() as u8);
            out.extend_from_slice(chunk);
        }
    }
    out
}

/// The items by tag, a split value joined again. `None` if the data ends
/// inside an item.
pub fn read(data: &[u8]) -> Option<HashMap<u8, Vec<u8>>> {
    let mut out: HashMap<u8, Vec<u8>> = HashMap::new();
    let mut rest = data;
    while let [tag, len, tail @ ..] = rest {
        let len = usize::from(*len);
        if tail.len() < len {
            return None;
        }
        out.entry(*tag).or_default().extend_from_slice(&tail[..len]);
        rest = &tail[len..];
    }
    rest.is_empty().then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_long_value_is_split_and_joined_again() {
        let long = vec![7u8; 300];
        let bytes = write(&[(SEQ_NO, &[1]), (PUBLIC_KEY, &long)]);
        assert_eq!(bytes.len(), 3 + 2 + 255 + 2 + 45);
        let items = read(&bytes).unwrap();
        assert_eq!(items[&SEQ_NO], [1]);
        assert_eq!(items[&PUBLIC_KEY], long);
        assert!(read(&bytes[..bytes.len() - 1]).is_none());
    }
}
