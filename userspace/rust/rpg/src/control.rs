// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! The control file: `Field: value` lines, a field's value going on in
//! lines that start with a space (the description), as Debian's.
//!
//!   Package: hello
//!   Version: 1.0-1
//!   Architecture: x86_64          (aarch64, riscv64, or all)
//!   Kind: system                  (or app: FoxDen's, FOXDEN.md)
//!   Depends: rustica-base (>= 0.1), foo | bar
//!   Conflicts: ...   Provides: ...
//!   Maintainer: ...  Homepage: ...  Section: ...
//!   Installed-Size: 120           (KiB, written by rpg build)
//!   Description: a summary line
//!    and more lines, each starting with a space

use crate::version;

/// The architectures Rustux is built for, or is being ported to
/// (docs/PORTING.md). A package is for one of them, or for `all`.
pub const ARCHS: [&str; 3] = ["x86_64", "aarch64", "riscv64"];

#[derive(Clone, Debug, Default)]
pub struct Control {
    /// The fields in their order (the names as written)
    pub fields: Vec<(String, String)>,
}

/// A relation: `name`, `name (>= 1.0)`
#[derive(Clone, Debug, PartialEq)]
pub struct Rel {
    pub name: String,
    pub op: Option<(String, String)>,
}

impl Rel {
    pub fn text(&self) -> String {
        match &self.op {
            Some((op, v)) => format!("{} ({op} {v})", self.name),
            None => self.name.clone(),
        }
    }
}

pub fn valid_name(n: &str) -> bool {
    n.len() >= 2
        && n.len() <= 128
        && n.as_bytes()[0].is_ascii_alphanumeric()
        && n.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"+-.".contains(&b))
}

impl Control {
    pub fn parse(text: &str) -> Result<Control, String> {
        let mut c = Control::default();
        for (n, line) in text.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            if line.starts_with(' ') || line.starts_with('\t') {
                let Some(last) = c.fields.last_mut() else {
                    return Err(format!("line {}: a continuation line before any field", n + 1));
                };
                // " ." is an empty line inside the value
                let l = line.trim_start_matches([' ', '\t']).trim_end();
                last.1.push('\n');
                last.1.push_str(if l == "." { "" } else { l });
                continue;
            }
            let Some((k, v)) = line.split_once(':') else {
                return Err(format!("line {}: not \"Field: value\"", n + 1));
            };
            let k = k.trim();
            if k.is_empty() || k.contains(' ') {
                return Err(format!("line {}: a field name without spaces", n + 1));
            }
            if c.get(k).is_some() {
                return Err(format!("line {}: {k} twice", n + 1));
            }
            c.fields.push((k.to_string(), v.trim().to_string()));
        }
        Ok(c)
    }

    pub fn get(&self, k: &str) -> Option<&str> {
        self.fields.iter().find(|(f, _)| f.eq_ignore_ascii_case(k)).map(|(_, v)| v.as_str())
    }

    pub fn set(&mut self, k: &str, v: &str) {
        match self.fields.iter_mut().find(|(f, _)| f.eq_ignore_ascii_case(k)) {
            Some(f) => f.1 = v.to_string(),
            None => self.fields.push((k.to_string(), v.to_string())),
        }
    }

    pub fn remove(&mut self, k: &str) {
        self.fields.retain(|(f, _)| !f.eq_ignore_ascii_case(k));
    }

    pub fn name(&self) -> &str {
        self.get("Package").unwrap_or("")
    }
    pub fn version(&self) -> &str {
        self.get("Version").unwrap_or("")
    }
    pub fn arch(&self) -> &str {
        self.get("Architecture").unwrap_or("")
    }
    pub fn kind(&self) -> &str {
        self.get("Kind").unwrap_or("system")
    }
    pub fn summary(&self) -> &str {
        self.get("Description").map(|d| d.lines().next().unwrap_or("")).unwrap_or("")
    }

    pub fn text(&self) -> String {
        let mut s = String::new();
        for (k, v) in &self.fields {
            let mut lines = v.lines();
            s.push_str(&format!("{k}: {}\n", lines.next().unwrap_or("")));
            for l in lines {
                // An empty line inside a value is " ." (it would end the stanza)
                s.push_str(&format!(" {}\n", if l.is_empty() { "." } else { l }));
            }
        }
        s
    }

    /// The fields a package must have, with sane values
    pub fn check(&self) -> Result<(), String> {
        let name = self.get("Package").ok_or("no Package field")?;
        if !valid_name(name) {
            return Err(format!("{name}: a package name is lower-case letters, digits and + - . (2 or more)"));
        }
        let v = self.get("Version").ok_or("no Version field")?;
        if !version::valid(v) {
            return Err(format!("{v}: not a version (1.0, 0.1.3-1, 2:1.0~rc1)"));
        }
        let a = self.get("Architecture").ok_or("no Architecture field")?;
        if a != "all" && !ARCHS.contains(&a) {
            return Err(format!("{a}: the architecture is {} or all", ARCHS.join(", ")));
        }
        if !["system", "app"].contains(&self.kind()) {
            return Err(format!("{}: Kind is system or app", self.kind()));
        }
        if self.summary().trim().is_empty() {
            return Err("no Description (its first line is the summary)".into());
        }
        for f in ["Depends", "Conflicts", "Provides"] {
            if let Some(v) = self.get(f) {
                parse_rels(v).map_err(|e| format!("{f}: {e}"))?;
            }
        }
        Ok(())
    }

    /// A relation field: each clause a list of alternatives
    pub fn rels(&self, field: &str) -> Vec<Vec<Rel>> {
        self.get(field).and_then(|v| parse_rels(v).ok()).unwrap_or_default()
    }
}

/// `a (>= 1.0), b | c` into clauses of alternatives
pub fn parse_rels(v: &str) -> Result<Vec<Vec<Rel>>, String> {
    let mut out = Vec::new();
    for clause in v.split(',') {
        let clause = clause.trim();
        if clause.is_empty() {
            continue;
        }
        let mut alts = Vec::new();
        for alt in clause.split('|') {
            let alt = alt.trim();
            let (name, op) = match alt.split_once('(') {
                Some((n, rest)) => {
                    let inner = rest.strip_suffix(')').ok_or(format!("{alt}: no closing )"))?.trim();
                    let opend = inner.find(|c: char| !"<>=".contains(c)).unwrap_or(inner.len());
                    let (op, ver) = (inner[..opend].trim(), inner[opend..].trim());
                    if !["<<", "<=", "=", ">=", ">>"].contains(&op) {
                        return Err(format!("{alt}: the operator is <<, <=, =, >= or >>"));
                    }
                    if !version::valid(ver) {
                        return Err(format!("{alt}: {ver} is not a version"));
                    }
                    (n.trim(), Some((op.to_string(), ver.to_string())))
                }
                None => (alt, None),
            };
            if !valid_name(name) {
                return Err(format!("{alt}: not a package name"));
            }
            alts.push(Rel { name: name.to_string(), op });
        }
        out.push(alts);
    }
    Ok(out)
}

/// Stanzas separated by empty lines (the status file)
pub fn parse_stanzas(text: &str) -> Result<Vec<Control>, String> {
    let mut out = Vec::new();
    for s in text.split("\n\n") {
        if s.trim().is_empty() {
            continue;
        }
        out.push(Control::parse(s)?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_round_trip() {
        let t = "Package: hello\nVersion: 1.0-1\nArchitecture: all\nDepends: aa (>= 1.0), bb | cc\nDescription: says hello\n a longer text\n .\n after an empty line\n";
        let c = Control::parse(t).unwrap();
        c.check().unwrap();
        assert_eq!(c.summary(), "says hello");
        assert_eq!(c.rels("Depends").len(), 2);
        assert_eq!(c.rels("Depends")[1].len(), 2);
        assert_eq!(c.rels("Depends")[0][0].op, Some((">=".into(), "1.0".into())));
        let again = parse_stanzas(&c.text()).unwrap();
        assert_eq!(again[0].get("Description"), Some("says hello\na longer text\n\nafter an empty line"));
        assert_eq!(again[0].get("Description"), c.get("Description"));
    }

    #[test]
    fn architectures() {
        for a in ["x86_64", "aarch64", "riscv64", "all"] {
            let c = Control::parse(&format!("Package: hello\nVersion: 1\nArchitecture: {a}\nDescription: x\n")).unwrap();
            c.check().unwrap();
        }
    }

    #[test]
    fn bad_controls() {
        for t in [
            "Version: 1\nArchitecture: all\nDescription: x\n",
            "Package: Hello\nVersion: 1\nArchitecture: all\nDescription: x\n",
            "Package: hello\nVersion: v1\nArchitecture: all\nDescription: x\n",
            "Package: hello\nVersion: 1\nArchitecture: i386\nDescription: x\n",
            "Package: hello\nVersion: 1\nArchitecture: all\n",
            "Package: hello\nVersion: 1\nArchitecture: all\nDescription: x\nDepends: a (> 1\n",
            "Package: hello\nVersion: 1\nArchitecture: all\nDescription: x\nKind: driver\n",
        ] {
            let r = Control::parse(t).and_then(|c| c.check());
            assert!(r.is_err(), "{t}");
        }
    }
}
