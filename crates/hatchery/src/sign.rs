//! Ed25519 keys and egg signatures.
//!
//! A signature covers the package, version, file name, checksum and
//! publish time of an egg, so that none of them can be changed in an index
//! without the key.

pub use ed25519_dalek::{SigningKey, VerifyingKey};
use semver::Version;

use ed25519_dalek::{Signature, Signer};

use crate::error::{Error, Result};
use crate::fsx::{hex, unhex};
use crate::name::Name;
use crate::time::Timestamp;

const PREFIX: &str = "ed25519:";

fn key_bytes(s: &str, what: &str) -> Result<[u8; 32]> {
    let s = s.trim();
    let raw = s.strip_prefix(PREFIX).unwrap_or(s);
    unhex(raw)
        .and_then(|b| <[u8; 32]>::try_from(b).ok())
        .ok_or_else(|| Error::invalid(what, "must be ed25519: and 64 hex digits"))
}

/// Parses a public key: `ed25519:` and 64 hex digits, or the hex alone.
pub fn parse_verifying_key(s: &str) -> Result<VerifyingKey> {
    let b = key_bytes(s, "public key")?;
    VerifyingKey::from_bytes(&b)
        .map_err(|_| Error::invalid("public key", "not a valid Ed25519 key"))
}

/// Parses a signing key (its 32-byte seed): `ed25519:` and 64 hex digits,
/// or the hex alone.
pub fn parse_signing_key(s: &str) -> Result<SigningKey> {
    Ok(SigningKey::from_bytes(&key_bytes(s, "signing key")?))
}

/// `ed25519:<hex>`.
pub fn format_verifying_key(k: &VerifyingKey) -> String {
    format!("{PREFIX}{}", hex(k.as_bytes()))
}

/// `ed25519:<hex of the seed>`.
pub fn format_signing_key(k: &SigningKey) -> String {
    format!("{PREFIX}{}", hex(&k.to_bytes()))
}

/// The bytes a signature is made over: one line each for a fixed tag, the
/// package, the version, the file name, the checksum and the publish time.
pub fn message(
    name: &Name,
    version: &Version,
    file: &str,
    sha256: &str,
    published: &Timestamp,
) -> Vec<u8> {
    format!(
        "hatchery-egg-1\n{name}\n{version}\n{file}\n{}\n{published}\n",
        sha256.to_ascii_lowercase()
    )
    .into_bytes()
}

/// Signs a message; the signature as lowercase hex.
pub fn sign(key: &SigningKey, message: &[u8]) -> String {
    hex(&key.sign(message).to_bytes())
}

/// Checks a hex signature. The error says why it does not hold.
pub fn verify(
    key: &VerifyingKey,
    message: &[u8],
    signature: &str,
) -> std::result::Result<(), String> {
    let bytes = unhex(signature)
        .and_then(|b| <[u8; 64]>::try_from(b).ok())
        .ok_or_else(|| "the signature is not 128 hex digits".to_string())?;
    key.verify_strict(message, &Signature::from_bytes(&bytes))
        .map_err(|_| "it was not made by the registry's key over this egg".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signs_and_verifies() {
        let key = SigningKey::from_bytes(&[7; 32]);
        let text = format_signing_key(&key);
        let again = parse_signing_key(&text).unwrap();
        assert_eq!(again.to_bytes(), key.to_bytes());
        let public = parse_verifying_key(&format_verifying_key(&key.verifying_key())).unwrap();

        let name = Name::parse("aiviarium/claude").unwrap();
        let t = Timestamp::parse("2026-11-10T09:12:00Z").unwrap();
        let msg = message(
            &name,
            &Version::new(1, 4, 0),
            "claude-1.4.0.egg",
            "9F2C",
            &t,
        );
        let sig = sign(&key, &msg);
        verify(&public, &msg, &sig).unwrap();

        let later = message(
            &name,
            &Version::new(1, 4, 0),
            "claude-1.4.0.egg",
            "9f2c",
            &t.plus(std::time::Duration::from_secs(1)),
        );
        assert!(verify(&public, &later, &sig).is_err());
        assert!(verify(&public, &msg, "00").is_err());
        assert!(parse_verifying_key("ed25519:abc").is_err());
    }
}
