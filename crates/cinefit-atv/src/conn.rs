//! A TCP connection that speaks HTTP or RTSP, in the clear until a
//! pair-verify, then through a [`Session`].
//!
//! The messages are formed as pyatv forms them (MIT; `pyatv/support/http.py`,
//! `pyatv/support/rtsp.py`), which is what the Apple TV is known to accept.

use std::io::{Read, Write};
use std::net::TcpStream;

use crate::Error;
use crate::hap::Session;

/// A request or a response: the first line, the headers, the body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub first: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Message {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// A response's status code.
    pub fn code(&self) -> Option<u16> {
        self.first.split(' ').nth(1)?.parse().ok()
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = self.first.clone();
        for (k, v) in &self.headers {
            out.push_str(&format!("\r\n{k}: {v}"));
        }
        if !self.body.is_empty() {
            out.push_str(&format!("\r\nContent-Length: {}", self.body.len()));
        }
        out.push_str("\r\n\r\n");
        let mut out = out.into_bytes();
        out.extend_from_slice(&self.body);
        out
    }

    /// The first whole message in `buf`, and how many bytes it took.
    pub fn decode(buf: &[u8]) -> Result<Option<(Self, usize)>, Error> {
        let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") else {
            return Ok(None);
        };
        let head = std::str::from_utf8(&buf[..end])
            .map_err(|_| Error::Protocol("a message head is not UTF-8"))?;
        let mut lines = head.split("\r\n");
        let first = lines.next().unwrap_or_default().to_string();
        let headers: Vec<(String, String)> = lines
            .filter_map(|l| l.split_once(':'))
            .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
            .collect();
        let msg = Self {
            first,
            headers,
            body: Vec::new(),
        };
        let len: usize = msg
            .header("Content-Length")
            .map(|v| v.parse())
            .transpose()
            .map_err(|_| Error::Protocol("a bad Content-Length"))?
            .unwrap_or(0);
        let start = end + 4;
        let Some(body) = buf.get(start..start + len) else {
            return Ok(None);
        };
        Ok(Some((
            Self {
                body: body.to_vec(),
                ..msg
            },
            start + len,
        )))
    }
}

pub struct Conn {
    pub stream: TcpStream,
    session: Option<Session>,
    buf: Vec<u8>,
}

impl Conn {
    pub fn new(stream: TcpStream) -> Self {
        Self {
            stream,
            session: None,
            buf: Vec::new(),
        }
    }

    /// Everything from here on is encrypted.
    pub fn encrypt(&mut self, session: Session) {
        self.session = Some(session);
    }

    pub fn send(&mut self, bytes: &[u8]) -> Result<(), Error> {
        let wire = match &mut self.session {
            Some(s) => s.encrypt(bytes),
            None => bytes.to_vec(),
        };
        self.stream.write_all(&wire)?;
        Ok(())
    }

    /// More bytes, decrypted; an error when the other end has closed.
    fn fill(&mut self) -> Result<(), Error> {
        let mut chunk = [0u8; 4096];
        let n = self.stream.read(&mut chunk)?;
        if n == 0 {
            return Err(Error::Closed);
        }
        match &mut self.session {
            Some(s) => self.buf.extend(s.decrypt(&chunk[..n])?),
            None => self.buf.extend_from_slice(&chunk[..n]),
        }
        Ok(())
    }

    /// The next whole item `decode` finds, reading as needed.
    pub fn next<T>(
        &mut self,
        decode: impl Fn(&[u8]) -> Result<Option<(T, usize)>, Error>,
    ) -> Result<T, Error> {
        loop {
            if let Some((item, used)) = decode(&self.buf)? {
                self.buf.drain(..used);
                return Ok(item);
            }
            self.fill()?;
        }
    }

    /// A request, and its response; an error for anything but 200.
    pub fn exchange(&mut self, request: &Message) -> Result<Message, Error> {
        self.send(&request.encode())?;
        let response = self.next(Message::decode)?;
        match response.code() {
            Some(200) => Ok(response),
            // 470: the Apple TV does not know these credentials.
            Some(470) => Err(Error::Refused("the pairing is not known (470)".into())),
            code => Err(Error::Refused(format!(
                "{} answered {code:?}",
                request.first
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_message_round_trips_and_waits_for_its_body() {
        let m = Message {
            first: "RTSP/1.0 200 OK".into(),
            headers: vec![("CSeq".into(), "3".into())],
            body: b"hello".to_vec(),
        };
        let wire = m.encode();
        assert_eq!(Message::decode(&wire[..wire.len() - 1]).unwrap(), None);
        let (back, used) = Message::decode(&wire).unwrap().unwrap();
        assert_eq!(used, wire.len());
        assert_eq!(back.code(), Some(200));
        assert_eq!(back.header("cseq"), Some("3"));
        assert_eq!(back.header("content-length"), Some("5"));
        assert_eq!(back.body, b"hello");
    }
}
