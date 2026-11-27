// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! The sound service's volume and devices, for the panel and Settings
//! (soundd's text requests, <rustux/sound.h>; docs/SOUND.md)

extern "C" {
    fn rx_sound_control(req: *const u8, out: *mut u8, cap: usize) -> i32;
}

/// One request ("status", "volume 50", "volume +5", "mute", "unmute",
/// "toggle"); the answer without its final "ok", or Err (soundd's error,
/// "" when it is not running)
pub fn control(req: &str) -> Result<String, String> {
    let mut r = req.as_bytes().to_vec();
    r.push(0);
    let mut buf = vec![0u8; 8192];
    let n = unsafe { rx_sound_control(r.as_ptr(), buf.as_mut_ptr(), buf.len()) };
    if n < 0 {
        return Err(String::new());
    }
    buf.truncate(n as usize);
    let text = String::from_utf8_lossy(&buf).to_string();
    if let Some(e) = text.strip_prefix("error: ") {
        return Err(e.trim().to_string());
    }
    Ok(text.lines().filter(|l| *l != "ok").map(|l| format!("{l}\n")).collect())
}

/// A sound device and its outputs ("speaker on", "headphones unplugged")
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Device {
    pub name: String,
    pub in_use: bool,
    pub playing: bool,
    pub outputs: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Status {
    /// 0-100
    pub volume: i32,
    pub muted: bool,
    pub devices: Vec<Device>,
}

/// What soundd says; None when it is not running
pub fn status() -> Option<Status> {
    parse(&control("status").ok()?)
}

fn parse(text: &str) -> Option<Status> {
    let mut s = Status::default();
    for l in text.lines() {
        if let Some(v) = l.strip_prefix("volume ") {
            s.volume = v.trim().parse().unwrap_or(0);
        } else if let Some(v) = l.strip_prefix("muted ") {
            s.muted = v.trim() == "yes";
        } else if let Some(v) = l.strip_prefix("device ") {
            if v == "none" {
                continue;
            }
            let in_use = v.contains(" (in use)");
            let playing = v.ends_with(" playing");
            let name = v.replace(" (in use)", "").trim_end_matches(" playing").to_string();
            s.devices.push(Device { name, in_use, playing, outputs: Vec::new() });
        } else if let Some(v) = l.strip_prefix("outputs ") {
            if let Some(d) = s.devices.last_mut() {
                d.outputs = v.split(',').map(|o| o.trim().to_string()).filter(|o| !o.is_empty() && o != "-").collect();
            }
        }
    }
    Some(s)
}

/// Set the volume (0-100); the new status
pub fn set_volume(v: i32) -> Option<Status> {
    parse(&control(&format!("volume {}", v.clamp(0, 100))).ok()?)
}

pub fn set_muted(m: bool) -> Option<Status> {
    parse(&control(if m { "mute" } else { "unmute" }).ok()?)
}

/// The waves the panel's speaker shows for a volume: 0-3
pub fn waves(volume: i32) -> u8 {
    match volume {
        v if v <= 0 => 0,
        v if v < 34 => 1,
        v if v < 67 => 2,
        _ => 3,
    }
}
