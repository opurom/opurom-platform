//! The crypto capability: SHA-256 hashing, AES-256-GCM symmetric sealing and a
//! secure random source. Every function is pure from the caller's view: the
//! same key, nonce and plaintext always produce the same ciphertext, so tests
//! can assert fixed vectors. Only the random source is nondeterministic, by
//! design. Pure-Rust crates keep every cross-compile target free of a C
//! toolchain; the Android adapter reaches javax.crypto for the identical
//! primitives.

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Key, Nonce};

/// AES-GCM nonce length in bytes (96-bit, the recommended size).
pub const NONCE_LEN: usize = 12;
/// AES-256-GCM tag length in bytes appended to the ciphertext.
pub const TAG_LEN: usize = 16;
/// AES-256 key length in bytes.
pub const KEY_LEN: usize = 32;

/// Lowercase hex SHA-256 of `data`. A fixed, platform-independent digest: the
/// empty input and any other vector match the published SHA-256 test vectors.
pub fn sha256_hex(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(data);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// Typed crypto failures; a caller never gets a partial or fabricated result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CryptoError {
    /// The key was not exactly [`KEY_LEN`] bytes.
    BadKeyLen,
    /// The nonce was not exactly [`NONCE_LEN`] bytes.
    BadNonceLen,
    /// The ciphertext was too short to carry a tag, or authentication failed.
    BadCiphertext,
    /// The system random source could not produce bytes.
    RandomUnavailable,
}

impl std::fmt::Display for CryptoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = match self {
            CryptoError::BadKeyLen => "AES-256 key must be 32 bytes",
            CryptoError::BadNonceLen => "AES-GCM nonce must be 12 bytes",
            CryptoError::BadCiphertext => "ciphertext is truncated or failed authentication",
            CryptoError::RandomUnavailable => "secure random source is unavailable",
        };
        f.write_str(text)
    }
}

impl std::error::Error for CryptoError {}

fn cipher(key: &[u8]) -> Result<Aes256Gcm, CryptoError> {
    if key.len() != KEY_LEN {
        return Err(CryptoError::BadKeyLen);
    }
    Ok(Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key)))
}

fn nonce_bytes(value: &[u8]) -> Result<&[u8; NONCE_LEN], CryptoError> {
    value.try_into().map_err(|_| CryptoError::BadNonceLen)
}

/// Seal `plaintext` under `key` with `nonce`, returning `ciphertext || tag`. The
/// caller owns nonce uniqueness: a repeated (key, nonce) pair breaks GCM, so use
/// [`secure_random_nonce`] for real messages and a fixed nonce only in tests.
pub fn aes_gcm_seal(key: &[u8], nonce: &[u8], plaintext: &[u8]) -> Result<Vec<u8>, CryptoError> {
    let cipher = cipher(key)?;
    let nonce = Nonce::from_slice(nonce_bytes(nonce)?);
    cipher
        .encrypt(
            nonce,
            Payload {
                msg: plaintext,
                aad: b"",
            },
        )
        .map_err(|_| CryptoError::BadCiphertext)
}

/// Open a `ciphertext || tag` blob produced by [`aes_gcm_seal`]. Fails with
/// [`CryptoError::BadCiphertext`] when the tag does not authenticate.
pub fn aes_gcm_open(key: &[u8], nonce: &[u8], sealed: &[u8]) -> Result<Vec<u8>, CryptoError> {
    let cipher = cipher(key)?;
    let nonce = Nonce::from_slice(nonce_bytes(nonce)?);
    cipher
        .decrypt(
            nonce,
            Payload {
                msg: sealed,
                aad: b"",
            },
        )
        .map_err(|_| CryptoError::BadCiphertext)
}

/// Fill `out` with cryptographically secure random bytes from the system source.
pub fn secure_random(out: &mut [u8]) -> Result<(), CryptoError> {
    getrandom::getrandom(out).map_err(|_| CryptoError::RandomUnavailable)
}

/// A fresh 12-byte nonce from the secure random source, ready for [`aes_gcm_seal`].
pub fn secure_random_nonce() -> Result<[u8; NONCE_LEN], CryptoError> {
    let mut nonce = [0u8; NONCE_LEN];
    secure_random(&mut nonce)?;
    Ok(nonce)
}
