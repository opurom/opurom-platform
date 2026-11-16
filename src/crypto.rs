//! The crypto capability: SHA-256 hashing, AES-256-GCM symmetric sealing and a
//! secure random source. Every function is pure from the caller's view: the
//! same key, nonce and plaintext always produce the same ciphertext, so tests
//! can assert fixed vectors. Only the random source is nondeterministic, by
//! design. Backed by `ring`, so the same code runs on every host and the Android
//! adapter reaches javax.crypto for the identical primitives.

use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM};
use ring::rand::{SecureRandom, SystemRandom};

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

fn aead_key(key: &[u8]) -> Result<LessSafeKey, CryptoError> {
    if key.len() != KEY_LEN {
        return Err(CryptoError::BadKeyLen);
    }
    let unbound = UnboundKey::new(&AES_256_GCM, key).map_err(|_| CryptoError::BadKeyLen)?;
    Ok(LessSafeKey::new(unbound))
}

fn nonce(value: &[u8]) -> Result<Nonce, CryptoError> {
    let bytes: [u8; NONCE_LEN] = value.try_into().map_err(|_| CryptoError::BadNonceLen)?;
    Ok(Nonce::assume_unique_for_key(bytes))
}

/// Seal `plaintext` under `key` with `nonce`, returning `ciphertext || tag`. The
/// caller owns nonce uniqueness: a repeated (key, nonce) pair breaks GCM, so use
/// [`secure_random_nonce`] for real messages and a fixed nonce only in tests.
pub fn aes_gcm_seal(key: &[u8], nonce_bytes: &[u8], plaintext: &[u8]) -> Result<Vec<u8>, CryptoError> {
    let key = aead_key(key)?;
    let nonce = nonce(nonce_bytes)?;
    let mut in_out = plaintext.to_vec();
    key.seal_in_place_append_tag(nonce, Aad::empty(), &mut in_out)
        .map_err(|_| CryptoError::BadCiphertext)?;
    Ok(in_out)
}

/// Open a `ciphertext || tag` blob produced by [`aes_gcm_seal`]. Fails with
/// [`CryptoError::BadCiphertext`] when the tag does not authenticate.
pub fn aes_gcm_open(key: &[u8], nonce_bytes: &[u8], sealed: &[u8]) -> Result<Vec<u8>, CryptoError> {
    let key = aead_key(key)?;
    let nonce = nonce(nonce_bytes)?;
    if sealed.len() < TAG_LEN {
        return Err(CryptoError::BadCiphertext);
    }
    let mut in_out = sealed.to_vec();
    let plain = key
        .open_in_place(nonce, Aad::empty(), &mut in_out)
        .map_err(|_| CryptoError::BadCiphertext)?;
    Ok(plain.to_vec())
}

/// Fill `out` with cryptographically secure random bytes from the system source.
pub fn secure_random(out: &mut [u8]) -> Result<(), CryptoError> {
    SystemRandom::new()
        .fill(out)
        .map_err(|_| CryptoError::RandomUnavailable)
}

/// A fresh 12-byte nonce from the secure random source, ready for [`aes_gcm_seal`].
pub fn secure_random_nonce() -> Result<[u8; NONCE_LEN], CryptoError> {
    let mut nonce = [0u8; NONCE_LEN];
    secure_random(&mut nonce)?;
    Ok(nonce)
}
