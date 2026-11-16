//! Tests for the storage and crypto platform primitives: persistence round-trip,
//! AES-GCM symmetry, a fixed SHA-256 vector and deterministic ordering. Storage
//! writes under a fresh temp dir per test so cases never share state.

use std::path::PathBuf;

use opurom_platform::crypto::{
    aes_gcm_open, aes_gcm_seal, secure_random, secure_random_nonce, sha256_hex, CryptoError, KEY_LEN,
};
use opurom_platform::storage::Storage;

fn temp_root(tag: &str) -> PathBuf {
    let mut root = std::env::temp_dir();
    let unique = format!(
        "opurom-store-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    );
    root.push(unique);
    root
}

// ---------------------------------------------------------------- storage

#[test]
fn kv_set_get_remove_round_trip() {
    let store = Storage::open(temp_root("kv")).unwrap();
    assert_eq!(store.get("missing").unwrap(), None);
    store.set("theme", "dark").unwrap();
    store.set("lang", "en").unwrap();
    assert_eq!(store.get("theme").unwrap().as_deref(), Some("dark"));
    store.remove("theme").unwrap();
    assert_eq!(store.get("theme").unwrap(), None);
    // Removing an absent key is a no-op, not an error.
    store.remove("theme").unwrap();
    assert_eq!(store.get("lang").unwrap().as_deref(), Some("en"));
}

#[test]
fn list_is_sorted_and_covers_kv_and_objects() {
    let store = Storage::open(temp_root("list")).unwrap();
    store.set("zeta", "1").unwrap();
    store.set("alpha", "2").unwrap();
    store.set_object("beta", r#"{"b":2,"a":1}"#).unwrap();
    assert_eq!(store.list().unwrap(), vec!["alpha", "beta", "zeta"]);
}

#[test]
fn object_round_trip_is_canonical_json() {
    let store = Storage::open(temp_root("obj")).unwrap();
    // Insertion order differs from canonical order; the store sorts keys.
    store.set_object("cfg", r#"{ "z": 1, "a": [true, null, -2.5], "m": {"y":"x"} }"#).unwrap();
    let stored = store.get_object("cfg").unwrap().unwrap();
    assert_eq!(stored, r#"{"a":[true,null,-2.5],"m":{"y":"x"},"z":1}"#);
    assert_eq!(store.get_object("absent").unwrap(), None);
}

#[test]
fn clear_empties_kv_and_objects() {
    let store = Storage::open(temp_root("clear")).unwrap();
    store.set("k", "v").unwrap();
    store.set_object("o", r#"{"x":1}"#).unwrap();
    store.clear().unwrap();
    assert!(store.list().unwrap().is_empty());
    assert_eq!(store.get("k").unwrap(), None);
    assert_eq!(store.get_object("o").unwrap(), None);
}

#[test]
fn invalid_keys_are_rejected() {
    let store = Storage::open(temp_root("keys")).unwrap();
    for bad in ["", "has space", "../escape", "slash/key"] {
        assert!(store.set(bad, "v").is_err(), "key {bad:?} must be rejected");
    }
}

// ---------------------------------------------------------------- crypto

#[test]
fn sha256_matches_fixed_vectors() {
    assert_eq!(
        sha256_hex(b""),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    assert_eq!(
        sha256_hex(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

#[test]
fn aes_gcm_seal_open_is_symmetric() {
    let key = [7u8; KEY_LEN];
    let nonce = [9u8; 12];
    let plaintext = b"lan chat message";
    let sealed = aes_gcm_seal(&key, &nonce, plaintext).unwrap();
    // Seal is deterministic for a fixed (key, nonce): same bytes every call.
    assert_eq!(sealed, aes_gcm_seal(&key, &nonce, plaintext).unwrap());
    assert_ne!(&sealed[..], &plaintext[..]);
    let opened = aes_gcm_open(&key, &nonce, &sealed).unwrap();
    assert_eq!(opened, plaintext);
}

#[test]
fn aes_gcm_rejects_tamper_and_bad_lengths() {
    let key = [1u8; KEY_LEN];
    let nonce = [2u8; 12];
    let mut sealed = aes_gcm_seal(&key, &nonce, b"payload").unwrap();
    let last = sealed.len() - 1;
    sealed[last] ^= 0xFF;
    assert_eq!(aes_gcm_open(&key, &nonce, &sealed), Err(CryptoError::BadCiphertext));
    assert_eq!(aes_gcm_seal(&[0u8; 31], &nonce, b"x"), Err(CryptoError::BadKeyLen));
    assert_eq!(aes_gcm_seal(&key, &[0u8; 8], b"x"), Err(CryptoError::BadNonceLen));
    assert_eq!(aes_gcm_open(&key, &nonce, b"short"), Err(CryptoError::BadCiphertext));
}

#[test]
fn secure_random_fills_and_varies() {
    let mut a = [0u8; 32];
    let mut b = [0u8; 32];
    secure_random(&mut a).unwrap();
    secure_random(&mut b).unwrap();
    assert_ne!(a, b, "two 32-byte random draws must differ");
    assert_ne!(a, [0u8; 32], "the random source must not return all zeros");
    let nonce = secure_random_nonce().unwrap();
    assert_eq!(nonce.len(), 12);
}

#[test]
fn sealed_message_round_trips_with_a_random_nonce() {
    let key = [3u8; KEY_LEN];
    let nonce = secure_random_nonce().unwrap();
    let sealed = aes_gcm_seal(&key, &nonce, b"offline app state").unwrap();
    assert_eq!(aes_gcm_open(&key, &nonce, &sealed).unwrap(), b"offline app state");
}
