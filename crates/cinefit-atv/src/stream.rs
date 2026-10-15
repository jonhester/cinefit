//! The AirPlay 2 data stream that carries remote-control messages: frames of
//! a 32-byte header and a binary plist, `{"params": {"data": <bytes>}}`,
//! the bytes being protobuf messages each led by its length as a varint.
//!
//! As pyatv frames it (`pyatv/protocols/airplay/channels.py`): the header is
//! big-endian, the frame's whole size, a 12-byte type (`sync` or `rply` and
//! zeros), a 4-byte command (`comm` or zeros), a sequence number and four
//! bytes of padding. A `sync` frame is answered with a `rply` of the same
//! sequence number and nothing in it.

use std::io::Cursor;

use prost::Message;

use crate::Error;
use crate::proto::ProtocolMessage;

pub const HEADER: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub kind: [u8; 12],
    pub command: [u8; 4],
    pub seqno: u64,
    pub payload: Vec<u8>,
}

impl Frame {
    pub fn is_sync(&self) -> bool {
        self.kind.starts_with(b"sync")
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(HEADER + self.payload.len());
        out.extend_from_slice(&((HEADER + self.payload.len()) as u32).to_be_bytes());
        out.extend_from_slice(&self.kind);
        out.extend_from_slice(&self.command);
        out.extend_from_slice(&self.seqno.to_be_bytes());
        out.extend_from_slice(&0u32.to_be_bytes());
        out.extend_from_slice(&self.payload);
        out
    }

    /// The first whole frame in `buf`, and how many bytes it took; `None`
    /// until one has arrived.
    pub fn decode(buf: &[u8]) -> Result<Option<(Self, usize)>, Error> {
        let Some(header) = buf.get(..HEADER) else {
            return Ok(None);
        };
        let size = u32::from_be_bytes(header[..4].try_into().expect("4 bytes")) as usize;
        if size < HEADER {
            return Err(Error::Protocol("a data frame shorter than its header"));
        }
        let Some(frame) = buf.get(..size) else {
            return Ok(None);
        };
        Ok(Some((
            Self {
                kind: header[4..16].try_into().expect("12 bytes"),
                command: header[16..20].try_into().expect("4 bytes"),
                seqno: u64::from_be_bytes(header[20..28].try_into().expect("8 bytes")),
                payload: frame[HEADER..].to_vec(),
            },
            size,
        )))
    }

    /// The reply a `sync` frame asks for.
    pub fn reply(seqno: u64) -> Self {
        Self {
            kind: pad(b"rply"),
            command: [0; 4],
            seqno,
            payload: Vec::new(),
        }
    }

    /// One message, sent as pyatv sends it.
    pub fn message(seqno: u64, msg: &ProtocolMessage) -> Self {
        let mut data = Vec::new();
        msg.encode_length_delimited(&mut data)
            .expect("a Vec grows to fit");
        let params = plist::Dictionary::from_iter([("data", plist::Value::Data(data))]);
        let body = plist::Value::Dictionary(plist::Dictionary::from_iter([(
            "params",
            plist::Value::Dictionary(params),
        )]));
        let mut payload = Vec::new();
        body.to_writer_binary(&mut payload)
            .expect("a Vec grows to fit");
        Self {
            kind: pad(b"sync"),
            command: *b"comm",
            seqno,
            payload,
        }
    }

    /// The messages a frame carries; none for a frame without `params.data`
    /// (a reply, or something else this does not read).
    pub fn messages(&self) -> Result<Vec<ProtocolMessage>, Error> {
        if self.payload.is_empty() {
            return Ok(Vec::new());
        }
        let value = plist::Value::from_reader(Cursor::new(&self.payload))
            .map_err(|_| Error::Protocol("a data frame's payload is not a plist"))?;
        let Some(data) = value
            .as_dictionary()
            .and_then(|d| d.get("params"))
            .and_then(plist::Value::as_dictionary)
            .and_then(|p| p.get("data"))
            .and_then(plist::Value::as_data)
        else {
            return Ok(Vec::new());
        };
        decode_messages(data)
    }
}

fn pad(s: &[u8; 4]) -> [u8; 12] {
    let mut k = [0u8; 12];
    k[..4].copy_from_slice(s);
    k
}

/// Length-led messages, or (as pyatv allows) one bare message: a message
/// starts with field 1's tag, 0x08, which no length of one does as its
/// varint would then be 8, too short for any message here.
fn decode_messages(mut data: &[u8]) -> Result<Vec<ProtocolMessage>, Error> {
    let bad = |_| Error::Protocol("a protobuf message did not decode");
    if data.first() == Some(&0x08) {
        return ProtocolMessage::decode(data).map(|m| vec![m]).map_err(bad);
    }
    let mut out = Vec::new();
    while !data.is_empty() {
        out.push(ProtocolMessage::decode_length_delimited(&mut data).map_err(bad)?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto;

    #[test]
    fn a_message_frame_round_trips() {
        let msg = proto::set_connection_state();
        let frame = Frame::message(0x1_2345_6789, &msg);
        let wire = frame.encode();
        assert_eq!(&wire[4..8], b"sync");
        assert_eq!(&wire[16..20], b"comm");
        // Not yet whole: nothing.
        assert_eq!(Frame::decode(&wire[..wire.len() - 1]).unwrap(), None);
        let mut two = wire.clone();
        two.extend(Frame::reply(7).encode());
        let (back, used) = Frame::decode(&two).unwrap().unwrap();
        assert_eq!(used, wire.len());
        assert!(back.is_sync());
        assert_eq!(back.messages().unwrap(), vec![msg]);
        let (reply, _) = Frame::decode(&two[used..]).unwrap().unwrap();
        assert_eq!((reply.seqno, reply.is_sync()), (7, false));
        assert!(reply.messages().unwrap().is_empty());
    }

    #[test]
    fn several_messages_or_one_bare_one_decode() {
        let a = proto::set_connection_state();
        let b = proto::get_keyboard_session();
        let mut data = Vec::new();
        a.encode_length_delimited(&mut data).unwrap();
        b.encode_length_delimited(&mut data).unwrap();
        assert_eq!(decode_messages(&data).unwrap(), vec![a.clone(), b]);
        assert_eq!(decode_messages(&a.encode_to_vec()).unwrap(), vec![a]);
    }
}
