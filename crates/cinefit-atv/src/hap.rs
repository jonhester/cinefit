//! The HomeKit-style pairing the Apple TV uses (HAP): the credentials one
//! pairing leaves behind, the pair-verify that proves them on each
//! connection, and the encryption it sets up.
//!
//! As pyatv implements it (MIT; `pyatv/auth/hap_srp.py`, `hap_session.py`):
//! X25519 for the session's shared secret, Ed25519 signatures with the
//! paired long-term keys, HKDF-SHA512 for keys, ChaCha20-Poly1305 with a
//! 12-byte nonce of four zero bytes and then eight of counter or label.

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier as _, VerifyingKey};
use hkdf::Hkdf;
use sha2::Sha512;
use x25519_dalek::{PublicKey, StaticSecret};

use crate::Error;
use crate::tlv8;

/// What one pairing leaves: the Apple TV's long-term public key, ours
/// (the secret half, an Ed25519 seed), its identifier and ours.
#[derive(Clone, PartialEq, Eq)]
pub struct Credentials {
    pub ltpk: [u8; 32],
    pub ltsk: [u8; 32],
    pub atv_id: Vec<u8>,
    pub client_id: Vec<u8>,
}

impl std::fmt::Debug for Credentials {
    // The secret key stays out of logs.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credentials")
            .field("atv_id", &String::from_utf8_lossy(&self.atv_id))
            .field("client_id", &String::from_utf8_lossy(&self.client_id))
            .finish_non_exhaustive()
    }
}

impl Credentials {
    /// pyatv's form, four hex fields joined by colons: `ltpk:ltsk:atv_id:
    /// client_id`.
    pub fn parse(s: &str) -> Option<Self> {
        let parts: Vec<Vec<u8>> = s.trim().split(':').map(unhex).collect::<Option<_>>()?;
        let [ltpk, ltsk, atv_id, client_id] = <[Vec<u8>; 4]>::try_from(parts).ok()?;
        Some(Self {
            ltpk: ltpk.try_into().ok()?,
            ltsk: ltsk.try_into().ok()?,
            atv_id,
            client_id,
        })
    }
}

impl Credentials {
    /// pyatv's form again, for keeping in a file.
    pub fn to_text(&self) -> String {
        [&self.ltpk[..], &self.ltsk, &self.atv_id, &self.client_id]
            .map(|b| b.iter().map(|x| format!("{x:02x}")).collect::<String>())
            .join(":")
    }
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

/// HKDF-SHA512 to a 32-byte key.
pub fn derive(salt: &str, info: &str, secret: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    Hkdf::<Sha512>::new(Some(salt.as_bytes()), secret)
        .expand(info.as_bytes(), &mut out)
        .expect("32 bytes is a valid HKDF-SHA512 length");
    out
}

/// Four zero bytes, then eight: a counter (little-endian) or a label.
fn nonce(last8: [u8; 8]) -> Nonce {
    let mut n = [0u8; 12];
    n[4..].copy_from_slice(&last8);
    *Nonce::from_slice(&n)
}

pub(crate) fn seal(key: &[u8; 32], last8: [u8; 8], aad: &[u8], data: &[u8]) -> Vec<u8> {
    ChaCha20Poly1305::new(Key::from_slice(key))
        .encrypt(&nonce(last8), Payload { msg: data, aad })
        .expect("ChaCha20-Poly1305 encryption does not fail")
}

pub(crate) fn open(
    key: &[u8; 32],
    last8: [u8; 8],
    aad: &[u8],
    data: &[u8],
) -> Result<Vec<u8>, Error> {
    ChaCha20Poly1305::new(Key::from_slice(key))
        .decrypt(&nonce(last8), Payload { msg: data, aad })
        .map_err(|_| Error::Crypto("a message did not decrypt"))
}

/// One pair-verify: two messages each way, then keys for the connection.
pub struct PairVerify {
    secret: StaticSecret,
    public: [u8; 32],
    shared: Option<[u8; 32]>,
}

impl PairVerify {
    /// A fresh session key pair, and the first message (M1).
    pub fn start() -> Result<(Self, Vec<u8>), Error> {
        let mut seed = [0u8; 32];
        getrandom::getrandom(&mut seed).map_err(|_| Error::Crypto("no random bytes"))?;
        let secret = StaticSecret::from(seed);
        let public = PublicKey::from(&secret).to_bytes();
        let m1 = tlv8::write(&[(tlv8::SEQ_NO, &[1]), (tlv8::PUBLIC_KEY, &public)]);
        Ok((
            Self {
                secret,
                public,
                shared: None,
            },
            m1,
        ))
    }

    /// The Apple TV's answer (M2): checks it is the paired device, and
    /// returns our proof (M3).
    pub fn answer(&mut self, creds: &Credentials, m2: &[u8]) -> Result<Vec<u8>, Error> {
        let items = tlv8::read(m2).ok_or(Error::Protocol("pair-verify M2 is not TLV8"))?;
        if let Some(e) = items.get(&tlv8::ERROR) {
            return Err(Error::Refused(format!("pair-verify error {e:?}")));
        }
        let their: [u8; 32] = items
            .get(&tlv8::PUBLIC_KEY)
            .and_then(|k| k.as_slice().try_into().ok())
            .ok_or(Error::Protocol("pair-verify M2 has no public key"))?;
        let encrypted = items
            .get(&tlv8::ENCRYPTED_DATA)
            .ok_or(Error::Protocol("pair-verify M2 has no encrypted data"))?;
        let shared = self
            .secret
            .diffie_hellman(&PublicKey::from(their))
            .to_bytes();
        let key = derive(
            "Pair-Verify-Encrypt-Salt",
            "Pair-Verify-Encrypt-Info",
            &shared,
        );
        let inner = open(&key, *b"PV-Msg02", &[], encrypted)?;
        let inner = tlv8::read(&inner).ok_or(Error::Protocol("pair-verify M2 inner TLV8"))?;
        let id = inner
            .get(&tlv8::IDENTIFIER)
            .ok_or(Error::Protocol("pair-verify M2 has no identifier"))?;
        if *id != creds.atv_id {
            return Err(Error::Refused("a different Apple TV answered".into()));
        }
        let sig: [u8; 64] = inner
            .get(&tlv8::SIGNATURE)
            .and_then(|s| s.as_slice().try_into().ok())
            .ok_or(Error::Protocol("pair-verify M2 has no signature"))?;
        let signed = [&their[..], id, &self.public].concat();
        VerifyingKey::from_bytes(&creds.ltpk)
            .and_then(|k| k.verify(&signed, &Signature::from_bytes(&sig)))
            .map_err(|_| Error::Refused("the Apple TV's signature does not match".into()))?;

        let ours = [&self.public[..], &creds.client_id, &their].concat();
        let sig = SigningKey::from_bytes(&creds.ltsk).sign(&ours).to_bytes();
        let proof = tlv8::write(&[
            (tlv8::IDENTIFIER, &creds.client_id),
            (tlv8::SIGNATURE, &sig),
        ]);
        let encrypted = seal(&key, *b"PV-Msg03", &[], &proof);
        self.shared = Some(shared);
        Ok(tlv8::write(&[
            (tlv8::SEQ_NO, &[3]),
            (tlv8::ENCRYPTED_DATA, &encrypted),
        ]))
    }

    /// Keys for a channel: (ours to write, ours to read).
    pub fn keys(&self, salt: &str, output: &str, input: &str) -> ([u8; 32], [u8; 32]) {
        let shared = self.shared.expect("keys come after a verified answer");
        (derive(salt, output, &shared), derive(salt, input, &shared))
    }
}

/// An encrypted byte stream: frames of at most 1024 bytes, each its length
/// (two bytes, little-endian, also the frame's associated data), then the
/// sealed bytes and their 16-byte tag. Each direction counts its frames.
pub struct Session {
    out_key: [u8; 32],
    in_key: [u8; 32],
    out_n: u64,
    in_n: u64,
    pending: Vec<u8>,
}

const FRAME: usize = 1024;
const TAG: usize = 16;

impl Session {
    pub fn new((out_key, in_key): ([u8; 32], [u8; 32])) -> Self {
        Self {
            out_key,
            in_key,
            out_n: 0,
            in_n: 0,
            pending: Vec::new(),
        }
    }

    pub fn encrypt(&mut self, data: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(data.len() + data.len() / FRAME * 18 + 18);
        for frame in data.chunks(FRAME) {
            let len = (frame.len() as u16).to_le_bytes();
            out.extend_from_slice(&len);
            out.extend(seal(&self.out_key, self.out_n.to_le_bytes(), &len, frame));
            self.out_n += 1;
        }
        out
    }

    /// Takes bytes as they arrive; returns what whole frames hold.
    pub fn decrypt(&mut self, data: &[u8]) -> Result<Vec<u8>, Error> {
        self.pending.extend_from_slice(data);
        let mut out = Vec::new();
        let mut used = 0;
        while let Some(&[a, b]) = self.pending.get(used..used + 2) {
            let len = usize::from(u16::from_le_bytes([a, b]));
            let end = used + 2 + len + TAG;
            let Some(sealed) = self.pending.get(used + 2..end) else {
                break;
            };
            out.extend(open(
                &self.in_key,
                self.in_n.to_le_bytes(),
                &[a, b],
                sealed,
            )?);
            self.in_n += 1;
            used = end;
        }
        self.pending.drain(..used);
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credentials_parse_from_pyatvs_form() {
        let s = format!(
            "{}:{}:{}:{}",
            "11".repeat(32),
            "22".repeat(32),
            "4142",
            "6364"
        );
        let c = Credentials::parse(&s).unwrap();
        assert_eq!(c.ltpk, [0x11; 32]);
        assert_eq!(c.ltsk, [0x22; 32]);
        assert_eq!(c.atv_id, b"AB");
        assert_eq!(c.client_id, b"cd");
        assert_eq!(Credentials::parse(&c.to_text()), Some(c.clone()));
        assert!(Credentials::parse("11:22").is_none());
        assert!(Credentials::parse(&s.replace("4142", "414")).is_none());
        // The secret key is not in the debug form.
        assert!(!format!("{c:?}").contains("22"));
    }

    #[test]
    fn a_session_round_trips_across_frames_and_split_reads() {
        let (a, b) = ([1u8; 32], [2u8; 32]);
        let mut tx = Session::new((a, b));
        let mut rx = Session::new((b, a));
        let data: Vec<u8> = (0..2500u32).map(|i| i as u8).collect();
        let wire = tx.encrypt(&data);
        assert_eq!(wire.len(), data.len() + 3 * (2 + TAG));
        // Delivered in awkward pieces, as TCP may.
        let mut got = Vec::new();
        for piece in wire.chunks(700) {
            got.extend(rx.decrypt(piece).unwrap());
        }
        assert_eq!(got, data);
        // The counters moved on together: the next message still opens.
        let more = tx.encrypt(b"again");
        assert_eq!(rx.decrypt(&more).unwrap(), b"again");
        // A changed byte is refused, not passed on.
        let mut bad = tx.encrypt(b"x");
        bad[3] ^= 1;
        assert!(rx.decrypt(&bad).is_err());
    }

    /// Pair-verify against a stand-in Apple TV doing its half as pyatv
    /// describes it: the keys both sides derive must agree.
    #[test]
    fn pair_verify_agrees_with_a_device_doing_its_half() {
        let atv_signing = SigningKey::from_bytes(&[5u8; 32]);
        let creds = Credentials {
            ltpk: atv_signing.verifying_key().to_bytes(),
            ltsk: [9u8; 32],
            atv_id: b"ATV-ID".to_vec(),
            client_id: b"CLIENT-ID".to_vec(),
        };
        let (mut pv, m1) = PairVerify::start().unwrap();
        let m1 = tlv8::read(&m1).unwrap();
        let client_pub: [u8; 32] = m1[&tlv8::PUBLIC_KEY].as_slice().try_into().unwrap();

        // The device: its own session key, a signature over both keys.
        let dev_secret = StaticSecret::from([7u8; 32]);
        let dev_pub = PublicKey::from(&dev_secret).to_bytes();
        let shared = dev_secret
            .diffie_hellman(&PublicKey::from(client_pub))
            .to_bytes();
        let key = derive(
            "Pair-Verify-Encrypt-Salt",
            "Pair-Verify-Encrypt-Info",
            &shared,
        );
        let sig = atv_signing
            .sign(&[&dev_pub[..], b"ATV-ID", &client_pub].concat())
            .to_bytes();
        let inner = tlv8::write(&[(tlv8::IDENTIFIER, b"ATV-ID"), (tlv8::SIGNATURE, &sig)]);
        let m2 = tlv8::write(&[
            (tlv8::SEQ_NO, &[2]),
            (tlv8::PUBLIC_KEY, &dev_pub),
            (tlv8::ENCRYPTED_DATA, &seal(&key, *b"PV-Msg02", &[], &inner)),
        ]);

        let m3 = pv.answer(&creds, &m2).unwrap();
        // The device checks our proof with our long-term public key.
        let m3 = tlv8::read(&m3).unwrap();
        let proof = open(&key, *b"PV-Msg03", &[], &m3[&tlv8::ENCRYPTED_DATA]).unwrap();
        let proof = tlv8::read(&proof).unwrap();
        assert_eq!(proof[&tlv8::IDENTIFIER], b"CLIENT-ID");
        let ours = SigningKey::from_bytes(&creds.ltsk).verifying_key();
        let sig: [u8; 64] = proof[&tlv8::SIGNATURE].as_slice().try_into().unwrap();
        ours.verify(
            &[&client_pub[..], b"CLIENT-ID", &dev_pub].concat(),
            &Signature::from_bytes(&sig),
        )
        .unwrap();
        // Both ends derive the same channel keys.
        let (w, r) = pv.keys(
            "Control-Salt",
            "Control-Write-Encryption-Key",
            "Control-Read-Encryption-Key",
        );
        assert_eq!(
            w,
            derive("Control-Salt", "Control-Write-Encryption-Key", &shared)
        );
        assert_eq!(
            r,
            derive("Control-Salt", "Control-Read-Encryption-Key", &shared)
        );

        // An answer made for another session does not open.
        let (mut pv2, _) = PairVerify::start().unwrap();
        let other = Credentials {
            atv_id: b"OTHER".to_vec(),
            ..creds
        };
        assert!(matches!(
            pv2.answer(&other, &m2),
            Err(Error::Refused(_)) | Err(Error::Crypto(_))
        ));
    }
}
