// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Package versions: `[EPOCH:]UPSTREAM[-REVISION]` (`0.1.3-1`), compared
//! as Debian compares them: the epoch as a number, then the upstream
//! version and the revision piece by piece, digits as numbers and the
//! rest as text, `~` sorting before everything (`1.0~rc1` < `1.0`).

use std::cmp::Ordering;

/// Is this a version? Starts with a digit; letters, digits and `.+~-`
/// (and `:` after an epoch)
pub fn valid(v: &str) -> bool {
    let (epoch, rest) = split_epoch(v);
    if epoch.is_none() && v.contains(':') {
        return false;
    }
    let (up, rev) = split_revision(rest);
    !up.is_empty()
        && up.as_bytes()[0].is_ascii_digit()
        && up.chars().all(|c| c.is_ascii_alphanumeric() || ".+~-".contains(c))
        && rev.chars().all(|c| c.is_ascii_alphanumeric() || ".+~".contains(c))
        && !(rest.ends_with('-'))
}

fn split_epoch(v: &str) -> (Option<u64>, &str) {
    match v.split_once(':') {
        Some((e, rest)) if !e.is_empty() && e.bytes().all(|b| b.is_ascii_digit()) => (e.parse().ok(), rest),
        _ => (None, v),
    }
}

fn split_revision(v: &str) -> (&str, &str) {
    match v.rfind('-') {
        Some(i) => (&v[..i], &v[i + 1..]),
        None => (v, ""),
    }
}

fn order(c: Option<u8>) -> i32 {
    match c {
        None => 0,
        Some(b'~') => -1,
        Some(c) if c.is_ascii_digit() => 0,
        Some(c) if c.is_ascii_alphabetic() => c as i32,
        Some(c) => c as i32 + 256,
    }
}

/// Debian's verrevcmp
fn cmp_part(a: &str, b: &str) -> Ordering {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let (mut i, mut j) = (0, 0);
    while i < a.len() || j < b.len() {
        let mut first_diff = 0;
        while (i < a.len() && !a[i].is_ascii_digit()) || (j < b.len() && !b[j].is_ascii_digit()) {
            let ac = order(a.get(i).copied().filter(|c| !c.is_ascii_digit()));
            let bc = order(b.get(j).copied().filter(|c| !c.is_ascii_digit()));
            if ac != bc {
                return ac.cmp(&bc);
            }
            if i < a.len() && !a[i].is_ascii_digit() {
                i += 1;
            }
            if j < b.len() && !b[j].is_ascii_digit() {
                j += 1;
            }
        }
        while i < a.len() && a[i] == b'0' {
            i += 1;
        }
        while j < b.len() && b[j] == b'0' {
            j += 1;
        }
        while i < a.len() && a[i].is_ascii_digit() && j < b.len() && b[j].is_ascii_digit() {
            if first_diff == 0 {
                first_diff = a[i] as i32 - b[j] as i32;
            }
            i += 1;
            j += 1;
        }
        if i < a.len() && a[i].is_ascii_digit() {
            return Ordering::Greater;
        }
        if j < b.len() && b[j].is_ascii_digit() {
            return Ordering::Less;
        }
        if first_diff != 0 {
            return first_diff.cmp(&0);
        }
    }
    Ordering::Equal
}

pub fn compare(a: &str, b: &str) -> Ordering {
    let (ea, ra) = split_epoch(a);
    let (eb, rb) = split_epoch(b);
    let o = ea.unwrap_or(0).cmp(&eb.unwrap_or(0));
    if o != Ordering::Equal {
        return o;
    }
    let (ua, va) = split_revision(ra);
    let (ub, vb) = split_revision(rb);
    cmp_part(ua, ub).then_with(|| cmp_part(va, vb))
}

/// A version constraint: `>= 1.0`, `<< 2`, `= 1.0-1`, ...
pub fn satisfies(have: &str, op: &str, want: &str) -> bool {
    let o = compare(have, want);
    match op {
        "<<" | "<" => o == Ordering::Less,
        "<=" => o != Ordering::Greater,
        "=" => o == Ordering::Equal,
        ">=" => o != Ordering::Less,
        ">>" | ">" => o == Ordering::Greater,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cmp::Ordering::*;

    #[test]
    fn debian_order() {
        for (a, b, o) in [
            ("1.0", "1.0", Equal),
            ("1.0", "1.1", Less),
            ("1.10", "1.9", Greater),
            ("1.0~rc1", "1.0", Less),
            ("1.0~rc1", "1.0~rc2", Less),
            ("1.0~~", "1.0~", Less),
            ("1.0-1", "1.0-2", Less),
            ("1.0-10", "1.0-9", Greater),
            ("1:0.1", "2.0", Greater),
            ("1.0a", "1.0", Greater),
            ("1.0+b1", "1.0", Greater),
            ("1.0.0", "1.0", Greater),
            ("0.1.3-1", "0.1.3", Greater),
            ("1.01", "1.1", Equal),
        ] {
            assert_eq!(compare(a, b), o, "{a} vs {b}");
            assert_eq!(compare(b, a), o.reverse(), "{b} vs {a}");
        }
    }

    #[test]
    fn validity() {
        for v in ["1", "0.1.3-1", "1:2.0", "1.0~rc1+git2-3"] {
            assert!(valid(v), "{v}");
        }
        for v in ["", "a1", "1.0-", "1 0", "x:1", "1.0/2"] {
            assert!(!valid(v), "{v}");
        }
    }
}
