// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! What to install for `rpg install NAME` and `rpg upgrade` (step E2).
//!
//! The packages asked for go in; then, for each dependency not met by
//! what is installed or going in, the first alternative a source can
//! satisfy, at the highest version that fits, goes in too (marked as
//! installed for another: `rpg autoremove` removes it once nothing needs
//! it). Then the whole result is checked: every package's dependencies
//! met, no conflicts. A held package (`rpg hold`) is never changed. For
//! `rpg upgrade`, an upgrade that cannot be made to fit is kept back and
//! the others go ahead.

use crate::control::{Control, Rel};
use crate::ops::satisfied;
use crate::repo::Avail;
use crate::version;
use std::collections::BTreeMap;

#[derive(Clone)]
pub struct Step {
    pub c: Control,
    /// From a source (None: a package file given on the command line,
    /// `file` its index)
    pub avail: Option<Avail>,
    pub file: Option<usize>,
    /// The installed version it replaces
    pub old: Option<String>,
    /// Some(true): installed for another package; Some(false): asked
    /// for; None: keep what the database says
    pub auto: Option<bool>,
    /// Why it is in (for a dependency: the package that needs it)
    pub why: Option<String>,
}

pub struct Plan {
    pub steps: Vec<Step>,
    /// Upgrades left out, with the reason
    pub kept: Vec<(String, String)>,
}

fn held(installed: &[Control], name: &str) -> bool {
    installed.iter().any(|c| c.name() == name && c.get("Hold") == Some("yes"))
}

/// The highest version a source has that satisfies `r` (by name; or a
/// package providing that name, for a relation without a version)
fn best_for<'a>(av: &'a [Avail], r: &Rel) -> Option<&'a Avail> {
    let mut best: Option<&Avail> = None;
    for a in av {
        let ok = if a.name() == r.name {
            match &r.op {
                Some((op, v)) => version::satisfies(a.version(), op, v),
                None => true,
            }
        } else {
            r.op.is_none() && a.c.rels("Provides").iter().flatten().any(|p| p.name == r.name)
        };
        if ok && best.is_none_or(|b| version::compare(a.version(), b.version()) == std::cmp::Ordering::Greater) {
            best = Some(a);
        }
    }
    best
}

pub fn resolve(installed: &[Control], av: &[Avail], targets: Vec<Step>) -> Result<Plan, String> {
    let mut world: BTreeMap<String, Control> = installed.iter().map(|c| (c.name().to_string(), c.clone())).collect();
    let mut steps: BTreeMap<String, Step> = BTreeMap::new();
    let mut queue: Vec<String> = Vec::new();
    for t in targets {
        let n = t.c.name().to_string();
        if held(installed, &n) && world.get(&n).is_some_and(|c| c.version() != t.c.version()) {
            return Err(format!("{n} is held at {} (rpg unhold {n})", world[&n].version()));
        }
        world.insert(n.clone(), t.c.clone());
        queue.push(n.clone());
        steps.insert(n, t);
    }
    while let Some(n) = queue.pop() {
        let c = world[&n].clone();
        for clause in c.rels("Depends") {
            let all: Vec<Control> = world.values().cloned().collect();
            if clause.iter().any(|r| satisfied(r, &all)) {
                continue;
            }
            let mut chosen = None;
            for r in &clause {
                if let Some(a) = best_for(av, r) {
                    if held(installed, a.name()) && world.contains_key(a.name()) {
                        continue;
                    }
                    chosen = Some(a.clone());
                    break;
                }
            }
            let Some(a) = chosen else {
                let what = clause.iter().map(Rel::text).collect::<Vec<_>>().join(" or ");
                let have = clause.iter().find_map(|r| world.get(&r.name)).map(|c| format!("; {} {} is installed", c.name(), c.version()));
                let held_note = clause.iter().find(|r| held(installed, &r.name)).map(|r| format!(" ({} is held)", r.name));
                return Err(format!(
                    "{} {} needs {what}, which no source has{}{}",
                    c.name(),
                    c.version(),
                    have.unwrap_or_default(),
                    held_note.unwrap_or_default()
                ));
            };
            let dn = a.name().to_string();
            let old = installed.iter().find(|i| i.name() == dn).map(|i| i.version().to_string());
            let auto = if old.is_some() { None } else { Some(true) };
            world.insert(dn.clone(), a.c.clone());
            steps.insert(dn.clone(), Step { c: a.c.clone(), avail: Some(a), file: None, old, auto, why: Some(n.clone()) });
            queue.push(dn);
        }
    }
    // The result as a whole
    let all: Vec<Control> = world.values().cloned().collect();
    for c in &all {
        for clause in c.rels("Depends") {
            if !clause.iter().any(|r| satisfied(r, &all)) {
                let what = clause.iter().map(Rel::text).collect::<Vec<_>>().join(" or ");
                let changed: Vec<String> = clause
                    .iter()
                    .filter_map(|r| steps.get(&r.name).map(|s| format!("{} {}", s.c.name(), s.c.version())))
                    .collect();
                return Err(format!(
                    "{} {} needs {what}{}",
                    c.name(),
                    c.version(),
                    if changed.is_empty() { String::new() } else { format!(", which {} would no longer be", changed.join(", ")) }
                ));
            }
        }
        for r in c.rels("Conflicts").iter().flatten() {
            for o in &all {
                if o.name() != c.name() && satisfied(r, std::slice::from_ref(o)) && (steps.contains_key(c.name()) || steps.contains_key(o.name())) {
                    return Err(format!("{} {} conflicts with {} {}", c.name(), c.version(), o.name(), o.version()));
                }
            }
        }
    }
    // Nothing to do for what is already as asked
    let steps: Vec<Step> = steps
        .into_values()
        .filter(|s| s.file.is_some() || s.old.as_deref() != Some(s.c.version()))
        .collect();
    Ok(Plan { steps, kept: Vec::new() })
}

/// rpg upgrade: every installed package a source has a newer version of
pub fn upgrade(installed: &[Control], av: &[Avail]) -> Plan {
    let mut targets = Vec::new();
    let mut kept = Vec::new();
    for c in installed {
        let Some(a) = crate::repo::candidate(av, c.name()) else { continue };
        if version::compare(a.version(), c.version()) != std::cmp::Ordering::Greater {
            continue;
        }
        if c.get("Hold") == Some("yes") {
            kept.push((c.name().to_string(), format!("held at {} ({} is there)", c.version(), a.version())));
            continue;
        }
        targets.push(Step { c: a.c.clone(), avail: Some(a.clone()), file: None, old: Some(c.version().to_string()), auto: None, why: None });
    }
    if let Ok(mut p) = resolve(installed, av, targets.clone()) {
        p.kept = kept;
        return p;
    }
    // Some upgrade does not fit: those that do, alone, then together
    let mut ok = Vec::new();
    for t in targets {
        match resolve(installed, av, vec![t.clone()]) {
            Ok(_) => ok.push(t),
            Err(e) => kept.push((t.c.name().to_string(), e)),
        }
    }
    loop {
        match resolve(installed, av, ok.clone()) {
            Ok(mut p) => {
                p.kept = kept;
                return p;
            }
            Err(e) => {
                // Drop the last one and try again
                match ok.pop() {
                    Some(t) => kept.push((t.c.name().to_string(), e)),
                    None => return Plan { steps: Vec::new(), kept },
                }
            }
        }
    }
}

/// rpg autoremove: packages installed for others that nothing installed
/// by the administrator needs any more
pub fn unneeded(installed: &[Control]) -> Vec<String> {
    let mut needed: std::collections::BTreeSet<String> = installed.iter().filter(|c| c.get("Auto") != Some("yes")).map(|c| c.name().to_string()).collect();
    let mut queue: Vec<String> = needed.iter().cloned().collect();
    while let Some(n) = queue.pop() {
        let Some(c) = installed.iter().find(|c| c.name() == n) else { continue };
        for clause in c.rels("Depends") {
            for r in &clause {
                if let Some(d) = installed.iter().find(|i| satisfied(r, std::slice::from_ref(*i))) {
                    if needed.insert(d.name().to_string()) {
                        queue.push(d.name().to_string());
                    }
                    break;
                }
            }
        }
    }
    installed.iter().map(|c| c.name().to_string()).filter(|n| !needed.contains(n)).collect()
}
