// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Dates and times in the user's chosen formats (2026-11-26), for the
//! panel's clock, Settings and Files. The choice is in
//! `~/.config/rustica.conf` (Settings > Date & Time writes it), else in
//! `/etc/rustica/datetime` (the system's default), else 24-hour and ISO:
//!
//! - `clock=24` or `clock=12` (with AM/PM)
//! - `seconds=yes|no`
//! - `date=iso` 2026-11-26, `us` 11/26/2026, `eu` 26/11/2026,
//!   `dots` 26.11.2026, `long` Thursday, November 26, 2026,
//!   `long-eu` Thursday 26 November 2026
//! - `panel_date=yes|no`: the panel shows the date under the time (yes by
//!   default since 2026-11-26, as Windows does)

use std::sync::Mutex;
use std::time::{Duration, Instant};

#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct Tm {
    pub sec: i32,
    pub min: i32,
    pub hour: i32,
    pub mday: i32,
    pub mon: i32,
    pub year: i32,
    pub wday: i32,
    pub yday: i32,
    pub isdst: i32,
    _rest: [i64; 4],
}

extern "C" {
    fn localtime_r(t: *const i64, tm: *mut Tm) -> *mut Tm;
    fn time(t: *mut i64) -> i64;
}

/// Seconds since 1970, in local time
pub fn local(t: i64) -> Tm {
    let mut tm = Tm::default();
    unsafe { localtime_r(&t, &mut tm) };
    tm
}

pub fn now() -> i64 {
    unsafe { time(std::ptr::null_mut()) }
}

const DAYS: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
const MONTHS: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];

/// The date formats, as `date=` names them, with an example each (for Settings)
pub const DATE_FORMATS: [(&str, &str); 6] = [
    ("iso", "2026-11-26"),
    ("us", "11/26/2026"),
    ("eu", "26/11/2026"),
    ("dots", "26.11.2026"),
    ("long", "Thursday, November 26, 2026"),
    ("long-eu", "Thursday 26 November 2026"),
];

#[derive(Clone, PartialEq, Debug)]
pub struct Formats {
    pub h12: bool,
    pub seconds: bool,
    pub date: String,
    pub panel_date: bool,
}

impl Default for Formats {
    fn default() -> Self {
        Formats { h12: false, seconds: false, date: "iso".into(), panel_date: true }
    }
}

fn kv(text: &str, key: &str) -> Option<String> {
    text.lines().find_map(|l| {
        let l = l.trim();
        let (k, v) = l.split_once('=')?;
        (k.trim() == key).then(|| v.trim().to_string())
    })
}

impl Formats {
    /// The user's choice over the system's
    pub fn load() -> Formats {
        let sys = std::fs::read_to_string("/etc/rustica/datetime").unwrap_or_default();
        let user = std::env::var("HOME").ok().and_then(|h| std::fs::read_to_string(format!("{h}/.config/rustica.conf")).ok()).unwrap_or_default();
        let get = |k: &str| kv(&user, k).or_else(|| kv(&sys, k));
        let mut f = Formats::default();
        if let Some(v) = get("clock") {
            f.h12 = v == "12";
        }
        if let Some(v) = get("seconds") {
            f.seconds = v == "yes";
        }
        if let Some(v) = get("date") {
            if DATE_FORMATS.iter().any(|(n, _)| *n == v) {
                f.date = v;
            }
        }
        if let Some(v) = get("panel_date") {
            f.panel_date = v == "yes";
        }
        f
    }

    /// Loaded again at most once a second (the panel asks every frame)
    pub fn current() -> Formats {
        static CACHE: Mutex<Option<(Instant, Formats)>> = Mutex::new(None);
        let mut c = CACHE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((t, f)) = c.as_ref() {
            if t.elapsed() < Duration::from_secs(1) {
                return f.clone();
            }
        }
        let f = Formats::load();
        *c = Some((Instant::now(), f.clone()));
        f
    }

    /// "14:05", "2:05 PM", with seconds if chosen
    pub fn time(&self, tm: &Tm) -> String {
        let s = if self.seconds { format!(":{:02}", tm.sec) } else { String::new() };
        if self.h12 {
            let h = match tm.hour % 12 {
                0 => 12,
                h => h,
            };
            format!("{}:{:02}{} {}", h, tm.min, s, if tm.hour < 12 { "AM" } else { "PM" })
        } else {
            format!("{:02}:{:02}{}", tm.hour, tm.min, s)
        }
    }

    /// The date in the chosen format
    pub fn date(&self, tm: &Tm) -> String {
        date_as(&self.date, tm)
    }

    /// A short date for the panel: "11/26", "26.11.", "Nov 26", ...
    pub fn short_date(&self, tm: &Tm) -> String {
        let (d, m, mon) = (tm.mday, tm.mon + 1, &MONTHS[tm.mon.rem_euclid(12) as usize][..3]);
        match self.date.as_str() {
            "us" => format!("{m}/{d}"),
            "eu" => format!("{d}/{m}"),
            "dots" => format!("{d}.{m}."),
            "long" => format!("{mon} {d}"),
            "long-eu" => format!("{d} {mon}"),
            _ => format!("{:02}-{:02}", m, d),
        }
    }

    /// A file's time in a list: the date (short forms for the long ones) and the time
    pub fn stamp(&self, t: i64) -> String {
        let tm = local(t);
        let d = match self.date.as_str() {
            "long" => format!("{} {}, {}", &MONTHS[tm.mon.rem_euclid(12) as usize][..3], tm.mday, tm.year + 1900),
            "long-eu" => format!("{} {} {}", tm.mday, &MONTHS[tm.mon.rem_euclid(12) as usize][..3], tm.year + 1900),
            _ => self.date(&tm),
        };
        let mut f = self.clone();
        f.seconds = false;
        format!("{} {}", d, f.time(&tm))
    }

    /// The panel's clock on two lines, as Windows' (2026-11-26): the time
    /// above the date in the chosen format ("2:05 PM" / "11/26/2026"; the
    /// long formats with the day and month shortened: "Thu, Nov 26, 2026").
    /// Without the date: one line, "Thu 14:05".
    pub fn panel_lines(&self, tm: &Tm) -> (String, Option<String>) {
        if !self.panel_date {
            return (self.panel(tm), None);
        }
        let day = &DAYS[tm.wday.rem_euclid(7) as usize][..3];
        let mon = &MONTHS[tm.mon.rem_euclid(12) as usize][..3];
        let y = tm.year + 1900;
        let date = match self.date.as_str() {
            "long" => format!("{day}, {mon} {}, {y}", tm.mday),
            "long-eu" => format!("{day} {} {mon} {y}", tm.mday),
            _ => self.date(tm),
        };
        (self.time(tm), Some(date))
    }

    /// The clock on one line: "Thu 14:05", or "Thu 11/26 2:05 PM" with the date
    pub fn panel(&self, tm: &Tm) -> String {
        let day = &DAYS[tm.wday.rem_euclid(7) as usize][..3];
        if self.panel_date {
            format!("{} {} {}", day, self.short_date(tm), self.time(tm))
        } else {
            format!("{} {}", day, self.time(tm))
        }
    }
}

/// A date in format `name` (one of DATE_FORMATS)
pub fn date_as(name: &str, tm: &Tm) -> String {
    let (y, m, d) = (tm.year + 1900, tm.mon + 1, tm.mday);
    let day = DAYS[tm.wday.rem_euclid(7) as usize];
    let month = MONTHS[tm.mon.rem_euclid(12) as usize];
    match name {
        "us" => format!("{:02}/{:02}/{}", m, d, y),
        "eu" => format!("{:02}/{:02}/{}", d, m, y),
        "dots" => format!("{:02}.{:02}.{}", d, m, y),
        "long" => format!("{day}, {month} {d}, {y}"),
        "long-eu" => format!("{day} {d} {month} {y}"),
        _ => format!("{:04}-{:02}-{:02}", y, m, d),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tm(h: i32, mi: i32) -> Tm {
        Tm { sec: 7, min: mi, hour: h, mday: 26, mon: 10, year: 126, wday: 4, ..Tm::default() }
    }

    #[test]
    fn formats() {
        let mut f = Formats::default();
        assert_eq!(f.panel_lines(&tm(14, 5)), ("14:05".into(), Some("2026-11-26".into())));
        f.panel_date = false;
        assert_eq!(f.panel(&tm(14, 5)), "Thu 14:05");
        assert_eq!(f.panel_lines(&tm(14, 5)), ("Thu 14:05".into(), None));
        f.h12 = true;
        assert_eq!(f.time(&tm(0, 5)), "12:05 AM");
        assert_eq!(f.time(&tm(12, 0)), "12:00 PM");
        assert_eq!(f.time(&tm(14, 5)), "2:05 PM");
        f.seconds = true;
        assert_eq!(f.time(&tm(14, 5)), "2:05:07 PM");
        f.seconds = false;
        f.date = "us".into();
        f.panel_date = true;
        assert_eq!(f.panel(&tm(14, 5)), "Thu 11/26 2:05 PM");
        assert_eq!(f.panel_lines(&tm(14, 5)), ("2:05 PM".into(), Some("11/26/2026".into())));
        f.date = "eu".into();
        assert_eq!(f.panel_lines(&tm(14, 5)).1.unwrap(), "26/11/2026");
        f.date = "long".into();
        assert_eq!(f.panel_lines(&tm(14, 5)).1.unwrap(), "Thu, Nov 26, 2026");
        assert_eq!(date_as("long", &tm(0, 0)), "Thursday, November 26, 2026");
        assert_eq!(date_as("long-eu", &tm(0, 0)), "Thursday 26 November 2026");
        assert_eq!(date_as("dots", &tm(0, 0)), "26.11.2026");
        assert_eq!(date_as("eu", &tm(0, 0)), "26/11/2026");
        assert_eq!(date_as("iso", &tm(0, 0)), "2026-11-26");
        for (n, ex) in DATE_FORMATS {
            assert_eq!(date_as(n, &tm(0, 0)), ex);
        }
    }
}
