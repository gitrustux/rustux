// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Trusted keys: /etc/rpg/keys/*.pub for system sources, and
//! /etc/rpg/keys/apps/*.pub for app sources (FoxDen, FOXDEN.md): a key
//! is trusted only for its kind. A key file is one line
//!
//!   rpg-ed25519 HEX-OF-THE-32-BYTE-PUBLIC-KEY NAME
//!
//! (`rpg key add` also takes OpenSSL's PEM public key and writes it in
//! this form). A key's ID is the first 8 bytes of the SHA-256 of the
//! public key, in hexadecimal. A Release file's signature (Release.sig)
//! is the 64-byte Ed25519 signature of its bytes, as
//! `openssl pkeyutl -sign -rawin` makes it.

use crate::db::Db;
use crate::pkg::sha256_hex;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use std::path::PathBuf;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Kind {
    System,
    App,
}

pub struct Key {
    pub name: String,
    pub key: [u8; 32],
    pub file: PathBuf,
}

impl Key {
    pub fn id(&self) -> String {
        sha256_hex(&self.key)[..16].to_string()
    }
    pub fn line(&self) -> String {
        format!("rpg-ed25519 {} {}", hex(&self.key), self.name)
    }
}

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
}

fn unbase64(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let (mut acc, mut bits) = (0u32, 0);
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' | b'\n' | b'\r' | b' ' => continue,
            _ => return None,
        };
        acc = acc << 6 | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

/// A key from a key file's text: ours, or a PEM public key (Ed25519's
/// SubjectPublicKeyInfo: 12 bytes of prefix, then the key)
pub fn parse(text: &str) -> Result<([u8; 32], Option<String>), String> {
    let t = text.trim();
    if t.starts_with("-----BEGIN PUBLIC KEY-----") {
        let body: String = t.lines().filter(|l| !l.starts_with("-----")).collect();
        let der = unbase64(&body).ok_or("a PEM key with bad base64")?;
        const PREFIX: [u8; 12] = [0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00];
        if der.len() != 44 || der[..12] != PREFIX {
            return Err("not an Ed25519 public key (openssl genpkey -algorithm ed25519 makes one)".into());
        }
        let mut k = [0u8; 32];
        k.copy_from_slice(&der[12..]);
        return Ok((k, None));
    }
    let line = t.lines().find(|l| !l.trim().is_empty() && !l.starts_with('#')).ok_or("an empty key file")?;
    let mut w = line.split_whitespace();
    if w.next() != Some("rpg-ed25519") {
        return Err("not a key (rpg-ed25519 HEX NAME, or a PEM public key)".into());
    }
    let b = w.next().and_then(unhex).filter(|b| b.len() == 32).ok_or("the key is not 64 hexadecimal digits")?;
    let mut k = [0u8; 32];
    k.copy_from_slice(&b);
    VerifyingKey::from_bytes(&k).map_err(|_| "not a valid Ed25519 public key")?;
    Ok((k, w.next().map(str::to_string)))
}

pub fn dir(db: &Db, kind: Kind) -> PathBuf {
    match kind {
        Kind::System => db.path("/etc/rpg/keys"),
        Kind::App => db.path("/etc/rpg/keys/apps"),
    }
}

/// The trusted keys of a kind (a bad key file is skipped, with a word)
pub fn load(db: &Db, kind: Kind) -> Vec<Key> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir(db, kind)) else { return out };
    let mut files: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "pub")).collect();
    files.sort();
    for f in files {
        match std::fs::read_to_string(&f).map_err(|e| e.to_string()).and_then(|t| parse(&t)) {
            Ok((key, name)) => {
                let name = name.unwrap_or_else(|| f.file_stem().unwrap_or_default().to_string_lossy().to_string());
                out.push(Key { name, key, file: f });
            }
            Err(e) => eprintln!("rpg: {}: {e} (skipped)", f.display()),
        }
    }
    out
}

/// Which key signed `data`, if any
pub fn verify<'a>(keys: &'a [Key], data: &[u8], sig: &[u8]) -> Option<&'a Key> {
    let sig: [u8; 64] = sig.try_into().ok()?;
    let sig = Signature::from_bytes(&sig);
    keys.iter().find(|k| VerifyingKey::from_bytes(&k.key).is_ok_and(|v| v.verify(data, &sig).is_ok()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_key() {
        // The real archive key (PACKAGES.md), both ways
        let (k, n) = parse("rpg-ed25519 b58e31b72c67a058bd41d9d7d93dda5fdfbda0d76daee09a4ea09affc208b3fe rustux-archive-2026\n").unwrap();
        assert_eq!(n.as_deref(), Some("rustux-archive-2026"));
        let key = Key { name: "x".into(), key: k, file: PathBuf::new() };
        assert_eq!(key.id(), "65527118d2c1e504");
        assert!(parse("rpg-ed25519 1234 x").is_err());
        assert!(parse("ssh-ed25519 AAAA x").is_err());
    }

    #[test]
    fn sign_and_verify() {
        use ed25519_dalek::{Signer, SigningKey};
        let sk = SigningKey::from_bytes(&[7u8; 32]);
        let key = Key { name: "t".into(), key: sk.verifying_key().to_bytes(), file: PathBuf::new() };
        let sig = sk.sign(b"Release text").to_bytes();
        assert!(verify(std::slice::from_ref(&key), b"Release text", &sig).is_some());
        assert!(verify(std::slice::from_ref(&key), b"Release text!", &sig).is_none());
        assert!(verify(std::slice::from_ref(&key), b"Release text", &sig[..63]).is_none());
    }
}
