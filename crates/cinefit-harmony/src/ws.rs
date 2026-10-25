//! A minimal RFC 6455 WebSocket client: just enough to send and receive text
//! frames over an already-connected stream. The Harmony Hub's command channel
//! is a WebSocket; its messages are small single text frames, so this handles
//! those, answers pings, and stops at a close. It does not fragment outgoing
//! messages or negotiate extensions, which the Hub does not ask for.
//!
//! Client-to-server frames are masked, as the spec requires; server-to-client
//! frames arrive unmasked.

use std::io::{self, Read, Write};

const OP_TEXT: u8 = 0x1;
const OP_CLOSE: u8 = 0x8;
const OP_PING: u8 = 0x9;
const OP_PONG: u8 = 0xA;

/// The largest message read, its frames together: the hub's whole
/// configuration is a few hundred KB, and a broken or hostile peer must not
/// make one frame allocate gigabytes (#231).
const MAX_MESSAGE: usize = 8 << 20;

pub struct WebSocket<S> {
    stream: S,
    mask_seed: u32,
}

impl<S: Read + Write> WebSocket<S> {
    /// Wraps a stream whose HTTP upgrade handshake has already completed.
    pub fn new(stream: S) -> Self {
        // A per-connection seed for mask keys. Not security: the mask only has
        // to vary, and the Hub does not care that it is not cryptographic.
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0x9e37_79b9);
        WebSocket {
            stream,
            mask_seed: seed | 1,
        }
    }

    /// The underlying stream, e.g. to adjust its read timeout.
    pub fn get_ref(&self) -> &S {
        &self.stream
    }

    fn next_mask(&mut self) -> [u8; 4] {
        // xorshift; any varying 32 bits will do.
        let mut x = self.mask_seed;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.mask_seed = x;
        x.to_le_bytes()
    }

    /// Sends one text frame.
    pub fn send_text(&mut self, text: &str) -> io::Result<()> {
        let payload = text.as_bytes();
        let mut frame = Vec::with_capacity(payload.len() + 14);
        frame.push(0x80 | OP_TEXT); // FIN + text

        let len = payload.len();
        if len < 126 {
            frame.push(0x80 | len as u8); // mask bit + length
        } else if len < 65536 {
            frame.push(0x80 | 126);
            frame.extend_from_slice(&(len as u16).to_be_bytes());
        } else {
            frame.push(0x80 | 127);
            frame.extend_from_slice(&(len as u64).to_be_bytes());
        }

        let mask = self.next_mask();
        frame.extend_from_slice(&mask);
        frame.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i & 3]));

        self.stream.write_all(&frame)?;
        self.stream.flush()
    }

    /// Reads the next text message, answering pings along the way. Returns
    /// `None` when the peer closes. Control frames other than ping/close are
    /// skipped.
    pub fn read_text(&mut self) -> io::Result<Option<String>> {
        loop {
            let (opcode, payload) = match self.read_frame()? {
                Some(f) => f,
                None => return Ok(None),
            };
            match opcode {
                OP_TEXT => {
                    return String::from_utf8(payload).map(Some).map_err(|_| {
                        io::Error::new(io::ErrorKind::InvalidData, "non-UTF-8 text frame")
                    });
                }
                OP_PING => self.send_control(OP_PONG, &payload)?,
                OP_CLOSE => {
                    // Echo the close and stop.
                    let _ = self.send_control(OP_CLOSE, &payload);
                    return Ok(None);
                }
                _ => continue, // pong, or a stray continuation: ignore.
            }
        }
    }

    fn send_control(&mut self, opcode: u8, payload: &[u8]) -> io::Result<()> {
        // Control payloads are short (<=125), so one length byte suffices.
        let mask = self.next_mask();
        let mut frame = Vec::with_capacity(payload.len() + 6);
        frame.push(0x80 | opcode);
        frame.push(0x80 | payload.len() as u8);
        frame.extend_from_slice(&mask);
        frame.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i & 3]));
        self.stream.write_all(&frame)?;
        self.stream.flush()
    }

    /// Reads one whole frame (reassembling a FIN-terminated run of
    /// continuation frames). Returns the first frame's opcode and the joined
    /// payload, or `None` at end of stream.
    fn read_frame(&mut self) -> io::Result<Option<(u8, Vec<u8>)>> {
        let mut first = [0u8; 2];
        if !self.read_exact_or_eof(&mut first)? {
            return Ok(None);
        }
        let opcode = first[0] & 0x0f;
        let mut fin = first[0] & 0x80 != 0;
        let mut payload = self.read_payload(first[1], MAX_MESSAGE)?;

        // Reassemble continuation frames (opcode 0) until FIN.
        while !fin {
            let mut hdr = [0u8; 2];
            if !self.read_exact_or_eof(&mut hdr)? {
                return Ok(None);
            }
            fin = hdr[0] & 0x80 != 0;
            let mut more = self.read_payload(hdr[1], MAX_MESSAGE - payload.len())?;
            payload.append(&mut more);
        }
        Ok(Some((opcode, payload)))
    }

    /// Reads a frame payload given its second header byte. Server frames are
    /// unmasked, so the mask bit is expected clear; if a server ever set it,
    /// the 4 mask bytes would be read and applied.
    /// `room` is what the message may still take: a length past it is an
    /// error before anything is allocated (#231).
    fn read_payload(&mut self, len_byte: u8, room: usize) -> io::Result<Vec<u8>> {
        let masked = len_byte & 0x80 != 0;
        let len = match len_byte & 0x7f {
            126 => {
                let mut b = [0u8; 2];
                self.stream.read_exact(&mut b)?;
                u64::from(u16::from_be_bytes(b))
            }
            127 => {
                let mut b = [0u8; 8];
                self.stream.read_exact(&mut b)?;
                u64::from_be_bytes(b)
            }
            n => u64::from(n),
        };
        let len = usize::try_from(len)
            .ok()
            .filter(|&l| l <= room)
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "a WebSocket frame of {len} bytes: messages are limited to {} MiB",
                        MAX_MESSAGE >> 20
                    ),
                )
            })?;
        let mask = if masked {
            let mut m = [0u8; 4];
            self.stream.read_exact(&mut m)?;
            Some(m)
        } else {
            None
        };
        let mut payload = vec![0u8; len];
        self.stream.read_exact(&mut payload)?;
        if let Some(m) = mask {
            for (i, b) in payload.iter_mut().enumerate() {
                *b ^= m[i & 3];
            }
        }
        Ok(payload)
    }

    /// Like `read_exact`, but returns `false` on a clean EOF at the start
    /// instead of an error.
    fn read_exact_or_eof(&mut self, buf: &mut [u8]) -> io::Result<bool> {
        let mut filled = 0;
        while filled < buf.len() {
            match self.stream.read(&mut buf[filled..])? {
                0 if filled == 0 => return Ok(false),
                0 => return Err(io::ErrorKind::UnexpectedEof.into()),
                n => filled += n,
            }
        }
        Ok(true)
    }
}

/// Base64-encodes bytes (standard alphabet, padded). Small and local so the
/// crate needs no base64 dependency just for the handshake key.
pub fn base64(input: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        out.push(A[(n >> 18 & 63) as usize] as char);
        out.push(A[(n >> 12 & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            A[(n >> 6 & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            A[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn base64_matches_known_vectors() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    /// A text frame we write should decode back to the same text, with the
    /// mask applied and the header shaped right.
    #[test]
    fn sent_text_frame_round_trips() {
        let mut ws = WebSocket::new(Cursor::new(Vec::new()));
        ws.send_text("hello").unwrap();
        let buf = ws.stream.into_inner();
        assert_eq!(buf[0], 0x81); // FIN + text
        assert_eq!(buf[1], 0x80 | 5); // masked, length 5
        let mask = &buf[2..6];
        let unmasked: Vec<u8> = buf[6..]
            .iter()
            .enumerate()
            .map(|(i, b)| b ^ mask[i & 3])
            .collect();
        assert_eq!(&unmasked, b"hello");
    }

    /// An unmasked server text frame should be read back verbatim.
    #[test]
    fn reads_an_unmasked_server_frame() {
        let mut raw = vec![0x81, 0x03]; // FIN+text, len 3, unmasked
        raw.extend_from_slice(b"abc");
        let mut ws = WebSocket::new(Cursor::new(raw));
        assert_eq!(ws.read_text().unwrap().as_deref(), Some("abc"));
    }

    #[test]
    fn reassembles_continuation_frames() {
        // "ab" split as text("a") + continuation("b"), FIN on the second.
        let raw = vec![0x01, 0x01, b'a', 0x80, 0x01, b'b'];
        let mut ws = WebSocket::new(Cursor::new(raw));
        assert_eq!(ws.read_text().unwrap().as_deref(), Some("ab"));
    }

    /// A frame declaring more than a message may hold is refused before its
    /// payload is allocated, alone or as the rest of a fragmented message.
    #[test]
    fn an_oversized_length_is_refused() {
        // FIN+text, a 64-bit length of 1 TiB, and nothing after it.
        let mut raw = vec![0x81, 127];
        raw.extend_from_slice(&(1u64 << 40).to_be_bytes());
        let mut ws = WebSocket::new(Cursor::new(raw));
        let e = ws.read_text().expect_err("too long");
        assert_eq!(e.kind(), io::ErrorKind::InvalidData, "{e}");
        // Two frames of 5 MiB each: the first fits, the two do not.
        let five = 5usize << 20;
        let mut raw = vec![0x01, 127];
        raw.extend_from_slice(&(five as u64).to_be_bytes());
        raw.resize(raw.len() + five, b'a');
        raw.extend_from_slice(&[0x80, 127]);
        raw.extend_from_slice(&(five as u64).to_be_bytes());
        let mut ws = WebSocket::new(Cursor::new(raw));
        let e = ws.read_text().expect_err("too long together");
        assert_eq!(e.kind(), io::ErrorKind::InvalidData, "{e}");
    }

    #[test]
    fn eof_reads_as_none() {
        let mut ws = WebSocket::new(Cursor::new(Vec::new()));
        assert_eq!(ws.read_text().unwrap(), None);
    }
}
