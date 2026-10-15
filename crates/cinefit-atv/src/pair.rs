//! The one-time pairing: the Apple TV shows a PIN, the PIN is typed in,
//! and both sides keep long-term keys that [`crate::hap::PairVerify`]
//! proves on each connection after.
//!
//! As pyatv's AirPlay pair-setup does it (MIT; `pyatv/protocols/airplay/
//! auth/hap.py`, `pyatv/auth/hap_srp.py`): `POST /pair-pin-start` shows the
//! PIN; `/pair-setup` then carries SRP (M1-M4, [`crate::srp`]), and our
//! signed identity out and the Apple TV's back, encrypted (M5, M6).

use ed25519_dalek::{Signer, SigningKey};

use crate::conn::{Conn, Message};
use crate::hap::{Credentials, derive, open, seal};
use crate::{Error, PORT, connect_to, srp, tlv8};

/// A pairing begun: the Apple TV is showing its PIN, and this connection
/// must stay open until it is typed in.
pub struct PairSetup {
    conn: Conn,
    /// Our new long-term key's seed, also the SRP private value as pyatv
    /// uses it.
    seed: [u8; 32],
    pairing_id: Vec<u8>,
    salt: Vec<u8>,
    b_pub: Vec<u8>,
}

fn post(path: &str, body: Vec<u8>) -> Message {
    Message {
        first: format!("POST {path} HTTP/1.1"),
        headers: vec![
            ("User-Agent".into(), "AirPlay/320.20".into()),
            ("Connection".into(), "keep-alive".into()),
            ("X-Apple-HKP".into(), "3".into()),
            ("Content-Type".into(), "application/octet-stream".into()),
        ],
        body,
    }
}

/// The device's TLV8 answer, or its error as a refusal.
fn answer(m: &Message, step: &str) -> Result<std::collections::HashMap<u8, Vec<u8>>, Error> {
    let items = tlv8::read(&m.body).ok_or(Error::Protocol("a pair-setup answer is not TLV8"))?;
    if let Some(e) = items.get(&tlv8::ERROR) {
        // 2 is authentication: the PIN was wrong.
        return Err(Error::Refused(if e.as_slice() == [2] {
            "the PIN was not right".into()
        } else {
            format!("pair-setup {step}: error {e:?}")
        }));
    }
    Ok(items)
}

/// Asks the Apple TV at `host` to show a PIN.
pub fn start(host: &str) -> Result<PairSetup, Error> {
    let mut conn = Conn::new(connect_to(host, PORT)?);
    conn.exchange(&post("/pair-pin-start", Vec::new()))?;
    let m2 = conn.exchange(&post(
        "/pair-setup",
        tlv8::write(&[(tlv8::METHOD, &[0]), (tlv8::SEQ_NO, &[1])]),
    ))?;
    let items = answer(&m2, "M2")?;
    let (Some(salt), Some(b_pub)) = (items.get(&tlv8::SALT), items.get(&tlv8::PUBLIC_KEY)) else {
        return Err(Error::Protocol("pair-setup M2 has no salt or public key"));
    };
    let mut seed = [0u8; 32];
    getrandom::getrandom(&mut seed).map_err(|_| Error::Crypto("no random bytes"))?;
    Ok(PairSetup {
        conn,
        seed,
        pairing_id: crate::uuid().to_lowercase().into_bytes(),
        salt: salt.clone(),
        b_pub: b_pub.clone(),
    })
}

impl PairSetup {
    /// The PIN the Apple TV showed: the pairing's credentials, to keep.
    pub fn finish(mut self, pin: &str) -> Result<Credentials, Error> {
        let c = srp::client(
            "Pair-Setup",
            pin.trim(),
            &self.seed,
            &self.salt,
            &self.b_pub,
        )
        .ok_or(Error::Protocol("the Apple TV's SRP value is not valid"))?;
        let m4 = self.conn.exchange(&post(
            "/pair-setup",
            tlv8::write(&[
                (tlv8::SEQ_NO, &[3]),
                (tlv8::PUBLIC_KEY, &c.public),
                (tlv8::PROOF, &c.proof),
            ]),
        ))?;
        let items = answer(&m4, "M4")?;
        // Its proof that it knows the PIN too: pyatv does not check it,
        // which this does.
        if items.get(&tlv8::PROOF).is_some_and(|p| *p != c.expected) {
            return Err(Error::Refused("the Apple TV's proof does not match".into()));
        }

        let signing = SigningKey::from_bytes(&self.seed);
        let ours = signing.verifying_key().to_bytes();
        let x = derive(
            "Pair-Setup-Controller-Sign-Salt",
            "Pair-Setup-Controller-Sign-Info",
            &c.key,
        );
        let key = derive("Pair-Setup-Encrypt-Salt", "Pair-Setup-Encrypt-Info", &c.key);
        let sig = signing
            .sign(&[&x[..], &self.pairing_id, &ours].concat())
            .to_bytes();
        let inner = tlv8::write(&[
            (tlv8::IDENTIFIER, &self.pairing_id),
            (tlv8::PUBLIC_KEY, &ours),
            (tlv8::SIGNATURE, &sig),
        ]);
        let m6 = self.conn.exchange(&post(
            "/pair-setup",
            tlv8::write(&[
                (tlv8::SEQ_NO, &[5]),
                (tlv8::ENCRYPTED_DATA, &seal(&key, *b"PS-Msg05", &[], &inner)),
            ]),
        ))?;
        let items = answer(&m6, "M6")?;
        let encrypted = items
            .get(&tlv8::ENCRYPTED_DATA)
            .ok_or(Error::Protocol("pair-setup M6 has no encrypted data"))?;
        let theirs = tlv8::read(&open(&key, *b"PS-Msg06", &[], encrypted)?)
            .ok_or(Error::Protocol("pair-setup M6 inner TLV8"))?;
        let (Some(atv_id), Some(ltpk)) = (
            theirs.get(&tlv8::IDENTIFIER),
            theirs
                .get(&tlv8::PUBLIC_KEY)
                .and_then(|k| <[u8; 32]>::try_from(k.as_slice()).ok()),
        ) else {
            return Err(Error::Protocol("pair-setup M6 has no identity"));
        };
        Ok(Credentials {
            ltpk,
            ltsk: self.seed,
            atv_id: atv_id.clone(),
            client_id: self.pairing_id,
        })
    }
}
