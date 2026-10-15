//! SRP-6a, the password check behind the one-time pairing: the Apple TV
//! shows a PIN, and each side proves it knows it without sending it.
//!
//! As pyatv does it (MIT; `pyatv/auth/hap_srp.py`), through srptools
//! (BSD-3-Clause, <https://github.com/idlesign/srptools>): the
//! 3072-bit group of RFC 5054 with generator 5, SHA-512, the user name
//! `Pair-Setup` and the PIN as the password. srptools' encodings are kept
//! exactly, since the Apple TV checks the result: numbers are hashed as
//! their shortest big-endian bytes, the salt as the bytes it came as
//! (a leading zero byte kept), A and B padded to the prime's length only
//! inside u, and g inside k.

use num_bigint::BigUint;
use sha2::{Digest, Sha512};

const N_HEX: [&str; 12] = [
    "FFFFFFFFFFFFFFFFC90FDAA22168C234C4C6628B80DC1CD129024E088A67CC74",
    "020BBEA63B139B22514A08798E3404DDEF9519B3CD3A431B302B0A6DF25F1437",
    "4FE1356D6D51C245E485B576625E7EC6F44C42E9A637ED6B0BFF5CB6F406B7ED",
    "EE386BFB5A899FA5AE9F24117C4B1FE649286651ECE45B3DC2007CB8A163BF05",
    "98DA48361C55D39A69163FA8FD24CF5F83655D23DCA3AD961C62F356208552BB",
    "9ED529077096966D670C354E4ABC9804F1746C08CA18217C32905E462E36CE3B",
    "E39E772C180E86039B2783A2EC07A28FB5C55DF06F4C52C9DE2BCBF695581718",
    "3995497CEA956AE515D2261898FA051015728E5A8AAAC42DAD33170D04507A33",
    "A85521ABDF1CBA64ECFB850458DBEF0A8AEA71575D060C7DB3970F85A6E1E4C7",
    "ABF5AE8CDB0933D71E8C94E04A25619DCEE3D2261AD2EE6BF12FFA06D98A0864",
    "D87602733EC86A64521F2B18177B200CBBE117577A615D6C770988C0BAD946E2",
    "08E24FA074E5AB3143DB5BFCE0FD108E4B82D120A93AD2CAFFFFFFFFFFFFFFFF",
];

fn n() -> BigUint {
    BigUint::parse_bytes(N_HEX.concat().as_bytes(), 16).expect("the prime is hex")
}

fn g() -> BigUint {
    BigUint::from(5u8)
}

/// srptools' int-to-bytes: the shortest big-endian form (zero is one byte).
fn bytes(v: &BigUint) -> Vec<u8> {
    v.to_bytes_be()
}

fn pad(v: &BigUint, len: usize) -> Vec<u8> {
    let b = bytes(v);
    let mut out = vec![0u8; len.saturating_sub(b.len())];
    out.extend(b);
    out
}

fn h(parts: &[&[u8]]) -> Vec<u8> {
    let mut d = Sha512::new();
    for p in parts {
        d.update(p);
    }
    d.finalize().to_vec()
}

fn int(b: &[u8]) -> BigUint {
    BigUint::from_bytes_be(b)
}

/// The client's half of one SRP exchange.
pub struct Client {
    /// Our public value, A, as sent.
    pub public: Vec<u8>,
    /// The shared session key, K.
    pub key: Vec<u8>,
    /// Our proof, M1, as sent.
    pub proof: Vec<u8>,
    /// The proof the device should answer with, M2.
    pub expected: Vec<u8>,
}

/// Runs the client's side against the device's salt and public value B,
/// with our private value `a` (bytes, as pyatv uses its signing key's).
/// `None` for a B that is zero modulo N, which would give away the key.
pub fn client(user: &str, password: &str, a: &[u8], salt: &[u8], b_pub: &[u8]) -> Option<Client> {
    let (n, g) = (n(), g());
    let len = bytes(&n).len();
    let b = int(b_pub);
    if (&b % &n) == BigUint::ZERO {
        return None;
    }
    let a = int(a);
    let k = int(&h(&[&bytes(&n), &pad(&g, len)]));
    let a_pub = g.modpow(&a, &n);
    let u = int(&h(&[&pad(&a_pub, len), &pad(&b, len)]));
    let inner = h(&[user.as_bytes(), b":", password.as_bytes()]);
    let x = int(&h(&[salt, &inner]));
    let v = g.modpow(&x, &n);
    // (B - k v) mod N, kept non-negative.
    let kv = (&k * &v) % &n;
    let base = ((&b % &n) + &n - kv) % &n;
    let s = base.modpow(&(&a + &u * &x), &n);
    let key = h(&[&bytes(&s)]);
    let hn_xor_hg = int(&h(&[&bytes(&n)])) ^ int(&h(&[&bytes(&g)]));
    let hu = bytes(&int(&h(&[user.as_bytes()])));
    let proof = h(&[
        &bytes(&hn_xor_hg),
        &hu,
        salt,
        &bytes(&a_pub),
        &bytes(&b),
        &key,
    ]);
    let expected = h(&[&bytes(&a_pub), &proof, &key]);
    Some(Client {
        public: bytes(&a_pub),
        key,
        proof,
        expected,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unhex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    /// Against srptools 1.0.1 (as pyatv 0.18 uses it), the values made with
    /// it on the rig, with a fixed private value, salt and
    /// device public value; the salt starts with a zero byte, which
    /// srptools keeps (it takes the salt as bytes, not as a number).
    #[test]
    fn the_numbers_agree_with_srptools() {
        let b = unhex(B);
        let salt = unhex(&format!("00ab{}", "cd".repeat(14)));
        let c = client("Pair-Setup", "1234", &[0x11; 32], &salt, &b).unwrap();
        assert_eq!(c.public, unhex(A));
        assert_eq!(c.key, unhex(K));
        assert_eq!(c.proof, unhex(M1));
        assert_eq!(c.expected, unhex(M2));
        // Another PIN is another proof.
        let wrong = client("Pair-Setup", "1235", &[0x11; 32], &salt, &b).unwrap();
        assert_ne!(wrong.proof, c.proof);
        assert!(client("Pair-Setup", "1234", &[0x11; 32], &salt, &bytes(&n())).is_none());
    }

    const B: &str = "3a788ae7ce130e509026b4a2cd69214e8fc0d370c8eaa30c9d688f3cd51ca59527121e89ed9a5b14a1b5fe30dd6e5b217db78ddee1d882e815daddf90bb2c67fe98f53f9f41903d94c3562bc53dfc3632ce902464b7659fe45cd3ea8ab3c0b86bc69b8bdfcff1edebcb068ec8f641ec5f5a909510f57150d86e4aec9f650c2588254aab9fc02b2ed3fa7a5365e811b0c19a0eb4a033cda54a0646c4036c950cefcefceb2c0a0413a4281c7802089ab30e56bb0407fa63e61cd66906aa55d11ba82b1c5eeafe0ed92e8edb1e5775371397a33223eb4298f3e2f4f0b4165ebe50ea4b4dc5eabaab8e665f43985125ab7513d2bd404fc2ee6818dc9e935176dcbb9847b335cb54ef8c0e13baf7a15ce365e5a62ce699bc03e3d3a98f12b097fbb3db0f139f520ef1f245d09da2ccda3ccb7b68cd111c82d54fe30e0d583a8084b8ed6da4311686c18151945bc62dd60e4f0ae7dd86199b2786b97753ca291b657db7ad7549682f6476888b74d89cfbecaf06cb6389aabadcf8911ac98153492e62d";
    const A: &str = "c95341372ca4d0b469f87c0ae37bda635713223b577ef5e8d64a959a54e5395722fea50e71bbd44306055760924a64885805ae13545fcfc7e65f4e7b75c765a112d267f79bffb8353d96ae81cfa64ca7eb687a5dba4cf223b21e1362489ca3e056254b25f610c00643490ea19944f6d3d0872bdcc5fc338bebc8936f30b695924e117f364a4cfc3898eeaa1a4bb98957df8eb046eba3cf42558138ca616a1dc7991426292dd258693bbd4c11816e3064e4a7c536f7255ae61735db25e3cf0349eb8b3e9848d0aa572b0f809d8983f1b280581aba96104422fcf088b698f8d899115a5653cc473e87f5e45552a5a126c8b7fc273bd2cd12bfb15ff3501a572b17608f16c33f6e971081f3af1be37a0a96748f60d204e935f6a5d2a71303aea434b45bcb9ab6b4444208d3fdf7a00c07a3ba2a09b56b578170c78ba1573ca2692406ba8d461e43bb3d160f0024a388d7234236537e29c3604f666c69d7244eae29a0e8b9fa5ee490890e5892973fa535707fbe8f06e90dcbb99c7b39b5eb2b45fb";
    const K: &str = "51a510a8a9a8f72e7296120b2367294bc9d0dc660f4fdd0c16c0df968173e917558120f2bd38e995138778579bb11ba76bfb51566542a6336416b6906417a8f2";
    const M1: &str = "b8d9c1a5dd24e8a63f70ca93ac67ba0bc5ae99f0928efc1881f4e0920a4b9fe2005b8e639dfdb7c8822a9c0e0fdd33de99264cfff7b52b16afa0588abffafa4e";
    const M2: &str = "6beff175b65d87c1c897e9e1347b401e06d89c1ff614f1fa8b6b52070c347d044adb86b81a491cca997c288a2fb601a8dfc28cdefd5175a67b113436d110ccbe";
}
