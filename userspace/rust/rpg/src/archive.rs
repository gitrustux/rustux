// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! The `.rpg` file: a gzipped tar (POSIX ustar, GNU long names), so the
//! host's `tar tzf` can look into one. Only what packages need: regular
//! files, directories and symbolic links.

use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use std::io::{Read, Write};

#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    File(Vec<u8>),
    Dir,
    Symlink(String),
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub path: String, // without a leading /
    pub mode: u32,
    pub kind: Kind,
}

fn octal(field: &mut [u8], v: u64) {
    let n = field.len() - 1;
    let s = format!("{:0w$o}", v, w = n);
    field[..n].copy_from_slice(s.as_bytes());
    field[n] = 0;
}

fn header(name: &[u8], mode: u32, size: u64, typ: u8, link: &[u8]) -> [u8; 512] {
    let mut h = [0u8; 512];
    // A name over 100 bytes went before as a GNU long name; here the
    // first 99
    let n = name.len().min(99);
    h[..n].copy_from_slice(&name[..n]);
    octal(&mut h[100..108], (mode & 0o7777) as u64);
    octal(&mut h[108..116], 0);
    octal(&mut h[116..124], 0);
    octal(&mut h[124..136], size);
    octal(&mut h[136..148], 0); // mtime: packages are the same whenever built
    h[156] = typ;
    let l = link.len().min(100);
    h[157..157 + l].copy_from_slice(&link[..l]);
    h[257..263].copy_from_slice(b"ustar\0");
    h[263..265].copy_from_slice(b"00");
    h[265..269].copy_from_slice(b"root");
    h[297..301].copy_from_slice(b"root");
    h[148..156].copy_from_slice(b"        ");
    let sum: u32 = h.iter().map(|&b| b as u32).sum();
    let s = format!("{:06o}\0 ", sum);
    h[148..156].copy_from_slice(s.as_bytes());
    h
}

fn pad(out: &mut Vec<u8>) {
    while out.len() % 512 != 0 {
        out.push(0);
    }
}

/// The tar of these entries, gzipped
pub fn write(entries: &[Entry]) -> Vec<u8> {
    let mut tar = Vec::new();
    for e in entries {
        let mut name = e.path.clone();
        if e.kind == Kind::Dir && !name.ends_with('/') {
            name.push('/');
        }
        let (typ, size, link): (u8, u64, &[u8]) = match &e.kind {
            Kind::File(d) => (b'0', d.len() as u64, b""),
            Kind::Dir => (b'5', 0, b""),
            Kind::Symlink(t) => (b'2', 0, t.as_bytes()),
        };
        if name.len() > 99 {
            let mut long = name.as_bytes().to_vec();
            long.push(0);
            tar.extend_from_slice(&header(b"././@LongLink", 0, long.len() as u64, b'L', b""));
            tar.extend_from_slice(&long);
            pad(&mut tar);
        }
        if link.len() > 99 {
            let mut long = link.to_vec();
            long.push(0);
            tar.extend_from_slice(&header(b"././@LongLink", 0, long.len() as u64, b'K', b""));
            tar.extend_from_slice(&long);
            pad(&mut tar);
        }
        tar.extend_from_slice(&header(name.as_bytes(), e.mode, size, typ, link));
        if let Kind::File(d) = &e.kind {
            tar.extend_from_slice(d);
            pad(&mut tar);
        }
    }
    tar.extend_from_slice(&[0u8; 1024]);
    let mut gz = GzEncoder::new(Vec::new(), Compression::best());
    gz.write_all(&tar).expect("gzip into memory");
    gz.finish().expect("gzip into memory")
}

fn field_str(f: &[u8]) -> String {
    let end = f.iter().position(|&b| b == 0).unwrap_or(f.len());
    String::from_utf8_lossy(&f[..end]).to_string()
}

fn field_octal(f: &[u8]) -> Result<u64, String> {
    let s = field_str(f);
    let s = s.trim_matches(|c: char| c == ' ' || c == '\0');
    if s.is_empty() {
        return Ok(0);
    }
    u64::from_str_radix(s, 8).map_err(|_| format!("a bad number in a tar header: {s:?}"))
}

/// The entries of a gzipped tar
pub fn read(gz: &[u8]) -> Result<Vec<Entry>, String> {
    let mut tar = Vec::new();
    GzDecoder::new(gz).read_to_end(&mut tar).map_err(|e| format!("not a gzipped file ({e})"))?;
    let mut out = Vec::new();
    let mut at = 0;
    let (mut long_name, mut long_link): (Option<String>, Option<String>) = (None, None);
    while at + 512 <= tar.len() {
        let h = &tar[at..at + 512];
        if h.iter().all(|&b| b == 0) {
            break;
        }
        let stored: u64 = field_octal(&h[148..156])?;
        let sum: u64 = h.iter().enumerate().map(|(i, &b)| if (148..156).contains(&i) { 32 } else { b as u64 }).sum();
        if stored != sum {
            return Err("a damaged tar header (checksum)".into());
        }
        let size = field_octal(&h[124..136])? as usize;
        let typ = h[156];
        let mut name = field_str(&h[..100]);
        if &h[257..262] == b"ustar" && h[345] != 0 {
            name = format!("{}/{}", field_str(&h[345..500]), name);
        }
        let link = field_str(&h[157..257]);
        let mode = field_octal(&h[100..108])? as u32;
        at += 512;
        let end = at.checked_add(size).filter(|&e| e <= tar.len()).ok_or("a truncated tar")?;
        let data = &tar[at..end];
        at += size.div_ceil(512) * 512;
        match typ {
            b'L' => {
                long_name = Some(field_str(data));
                continue;
            }
            b'K' => {
                long_link = Some(field_str(data));
                continue;
            }
            b'x' | b'g' => continue, // pax headers: nothing packages need
            _ => {}
        }
        let name = long_name.take().unwrap_or(name);
        let link = long_link.take().unwrap_or(link);
        let path = name.trim_start_matches("./").trim_end_matches('/').to_string();
        let kind = match typ {
            b'0' | 0 | b'7' => Kind::File(data.to_vec()),
            b'5' => Kind::Dir,
            b'2' => Kind::Symlink(link),
            t => return Err(format!("{path}: a tar entry of type {:?} (packages hold files, directories and symbolic links)", t as char)),
        };
        if path.is_empty() {
            continue;
        }
        out.push(Entry { path, mode, kind });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let long = format!("data/usr/share/{}/file", "x".repeat(150));
        let entries = vec![
            Entry { path: "control".into(), mode: 0o644, kind: Kind::File(b"Package: x\n".to_vec()) },
            Entry { path: "data/usr".into(), mode: 0o755, kind: Kind::Dir },
            Entry { path: long.clone(), mode: 0o600, kind: Kind::File(vec![7u8; 1000]) },
            Entry { path: "data/usr/lib/l".into(), mode: 0o777, kind: Kind::Symlink("y".repeat(120)) },
            Entry { path: "data/empty".into(), mode: 0o644, kind: Kind::File(Vec::new()) },
        ];
        let back = read(&write(&entries)).unwrap();
        assert_eq!(back.len(), entries.len());
        for (a, b) in entries.iter().zip(back.iter()) {
            assert_eq!(a.path, b.path);
            assert_eq!(a.mode, b.mode);
            assert_eq!(a.kind, b.kind);
        }
        assert!(read(b"not gzip").is_err());
    }
}
