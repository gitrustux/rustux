// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! calculator: Rustica's Calculator, an app (FoxDen step FD3, the apps
//! repository's org.rustica.calculator). Buttons or the keyboard: digits,
//! + - * / (× ÷), ( ), the decimal point, Enter or = for the result,
//! Backspace, Esc to clear. Usual precedence: 2+3*4 is 14. It asks for no
//! permissions. Each result is also printed ("calculator: 2+3*4 = 14").

use rustica_ui::gfx::Rect;
use rustica_ui::ui::keys;
use rustica_ui::{Control, Ui};

/// The expression as typed (ASCII operators; shown with × and ÷)
struct Calc {
    expr: String,
    result: Option<Result<f64, String>>,
}

// ---- evaluating ----

struct P<'a> {
    s: &'a [u8],
    i: usize,
}

impl P<'_> {
    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }
    // expr = term (("+" | "-") term)*
    fn expr(&mut self) -> Result<f64, String> {
        let mut v = self.term()?;
        while let Some(c) = self.peek() {
            match c {
                b'+' => {
                    self.i += 1;
                    v += self.term()?
                }
                b'-' => {
                    self.i += 1;
                    v -= self.term()?
                }
                _ => break,
            }
        }
        Ok(v)
    }
    // term = unary (("*" | "/") unary)*
    fn term(&mut self) -> Result<f64, String> {
        let mut v = self.unary()?;
        while let Some(c) = self.peek() {
            match c {
                b'*' => {
                    self.i += 1;
                    v *= self.unary()?
                }
                b'/' => {
                    self.i += 1;
                    let d = self.unary()?;
                    if d == 0.0 {
                        return Err("cannot divide by zero".into());
                    }
                    v /= d
                }
                _ => break,
            }
        }
        Ok(v)
    }
    fn unary(&mut self) -> Result<f64, String> {
        match self.peek() {
            Some(b'-') => {
                self.i += 1;
                Ok(-self.unary()?)
            }
            Some(b'+') => {
                self.i += 1;
                self.unary()
            }
            _ => self.atom(),
        }
    }
    fn atom(&mut self) -> Result<f64, String> {
        match self.peek() {
            Some(b'(') => {
                self.i += 1;
                let v = self.expr()?;
                if self.peek() != Some(b')') {
                    return Err("a ( without its )".into());
                }
                self.i += 1;
                Ok(v)
            }
            Some(c) if c.is_ascii_digit() || c == b'.' => {
                let st = self.i;
                while self.peek().is_some_and(|c| c.is_ascii_digit() || c == b'.') {
                    self.i += 1;
                }
                let t = std::str::from_utf8(&self.s[st..self.i]).unwrap();
                t.parse::<f64>().map_err(|_| format!("{t}: not a number"))
            }
            Some(_) => Err("an operator in the wrong place".into()),
            None => Err("unfinished".into()),
        }
    }
}

pub fn eval(expr: &str) -> Result<f64, String> {
    let s: Vec<u8> = expr.bytes().filter(|b| *b != b' ').collect();
    if s.is_empty() {
        return Err("nothing to work out".into());
    }
    let mut p = P { s: &s, i: 0 };
    let v = p.expr()?;
    if p.i != s.len() {
        return Err(if s[p.i] == b')' { "a ) without its (".into() } else { "an operator in the wrong place".into() });
    }
    if !v.is_finite() {
        return Err("too large".into());
    }
    Ok(v)
}

/// 14, 0.5, 3.333333333333, 1.5e20
pub fn show(v: f64) -> String {
    if v == v.trunc() && v.abs() < 1e15 {
        return format!("{}", v as i64);
    }
    if v.abs() >= 1e15 || v.abs() < 1e-9 {
        return format!("{v:e}");
    }
    let t = format!("{v:.12}");
    t.trim_end_matches('0').trim_end_matches('.').to_string()
}

fn pretty(expr: &str) -> String {
    expr.replace('*', "×").replace('/', "÷")
}

// ---- the window ----

const KEYS: [[&str; 4]; 5] = [["C", "(", ")", "÷"], ["7", "8", "9", "×"], ["4", "5", "6", "-"], ["1", "2", "3", "+"], ["0", ".", "⌫", "="]];

impl Calc {
    fn press(&mut self, k: &str) {
        match k {
            "C" => {
                self.expr.clear();
                self.result = None;
            }
            "⌫" => {
                self.expr.pop();
                self.result = None;
            }
            "=" => {
                let r = eval(&self.expr);
                match &r {
                    Ok(v) => println!("calculator: {} = {}", self.expr, show(*v)),
                    Err(e) => println!("calculator: {}: {e}", self.expr),
                }
                self.result = Some(r);
            }
            _ => {
                let c = match k {
                    "×" => "*",
                    "÷" => "/",
                    k => k,
                };
                // After a result, a digit starts afresh and an operator goes on from it
                if let Some(Ok(v)) = self.result.take() {
                    if "+-*/".contains(c) {
                        self.expr = show(v);
                    } else {
                        self.expr.clear();
                    }
                }
                if self.expr.len() < 120 {
                    self.expr.push_str(c);
                }
            }
        }
    }

    fn frame(&mut self, ui: &mut Ui, ctl: &mut Control) {
        let keys = std::mem::take(&mut ui.input.keys);
        for k in &keys {
            match k.usage {
                keys::ENTER | keys::KP_ENTER => self.press("="),
                keys::BACKSPACE => self.press("⌫"),
                keys::ESC | keys::DELETE => self.press("C"),
                _ => {
                    let t = k.text.as_str();
                    if k.ctrl() {
                        if k.usage == 0x14 {
                            ctl.quit = true; // Ctrl+Q
                        }
                    } else if t.len() == 1 && "0123456789.+-*/()".contains(t) {
                        self.press(t);
                    } else if t == "=" {
                        self.press("=");
                    } else if t == "x" || t == "X" {
                        self.press("×");
                    }
                }
            }
        }
        // The display: the expression above, the result (or what is wrong) below
        let w = ui.available_width();
        let h = ui.u(84.0);
        let r = ui.alloc(w, h);
        let th = ui.theme;
        ui.canvas.fill_rounded(r, ui.u(8.0), th.surface);
        let small = ui.font_px();
        let big = ui.font_px() * 2.2;
        let top = pretty(&self.expr);
        let (line, line_c) = match &self.result {
            Some(Ok(v)) => (show(*v), th.text),
            Some(Err(e)) => (e.clone(), ui.theme.danger),
            None => (if top.is_empty() { "0".to_string() } else { top.clone() }, th.text),
        };
        let shown_top = if self.result.is_some() { format!("{top} =") } else { String::new() };
        let tw = ui.face.width(&shown_top, small);
        let (m12, m8, m4) = (ui.u(12.0), ui.u(8.0), ui.u(4.0));
        ui.canvas.text(&mut ui.face, r.x + r.w - tw - m12, r.y + m8, &shown_top, small, th.dim);
        let px = if line.chars().count() > 14 { big * 14.0 / line.chars().count() as f32 } else { big };
        let bw = ui.bold.width(&line, px);
        ui.canvas.text(&mut ui.bold, r.x + r.w - bw - m12, r.y + r.h - px * 1.3 - m4, &line, px, line_c);
        // The keys: a grid of equal buttons drawn here (the toolkit's
        // buttons are as wide as their labels)
        let gap = ui.u(8.0);
        let rows = KEYS.len() as f32;
        let gh = ui.remaining_height();
        let kw = ((w - gap * 3.0) / 4.0).floor();
        let kh = ((gh - gap * (rows - 1.0)) / rows).floor().max(ui.u(36.0));
        let area = ui.alloc(w, kh * rows + gap * (rows - 1.0));
        let (mx, my) = ui.input.mouse;
        let released = ui.input.released;
        let down = ui.input.down;
        for (ri, row) in KEYS.iter().enumerate() {
            for (ci, k) in row.iter().enumerate() {
                let kr = Rect::new(area.x + ci as f32 * (kw + gap), area.y + ri as f32 * (kh + gap), kw, kh);
                let hot = kr.contains(mx, my);
                let op = ci == 3 || ri == 0;
                let eq = *k == "=";
                let bg = if eq {
                    if hot { rustica_ui::ui::mix(th.accent, 0xFFFFFF, 0.22) } else { th.accent }
                } else if hot && down {
                    th.selection
                } else if hot {
                    th.surface_hi
                } else if op {
                    rustica_ui::ui::mix(th.surface, th.surface_hi, 0.5)
                } else {
                    th.surface
                };
                ui.canvas.fill_rounded(kr, m8, bg);
                let px = ui.font_px() * 1.3;
                let face = if eq { &mut ui.bold } else { &mut ui.face };
                let lw = face.width(k, px);
                let fg = if eq { th.accent_text } else if op { th.accent } else { th.text };
                ui.canvas.text(face, kr.x + (kr.w - lw) / 2.0, kr.y + (kr.h - px * 1.25) / 2.0, k, px, fg);
                if hot && released {
                    self.press(k);
                }
            }
        }
    }
}

fn main() {
    let mut c = Calc { expr: String::new(), result: None };
    if let Err(e) = rustica_ui::run("Calculator", 320, 440, None, |ui, ctl| c.frame(ui, ctl)) {
        eprintln!("calculator: {e}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arithmetic() {
        assert_eq!(eval("2+3*4").unwrap(), 14.0);
        assert_eq!(eval("(2+3)*4").unwrap(), 20.0);
        assert_eq!(eval("-3+10/4").unwrap(), -0.5);
        assert_eq!(eval("1.5*2").unwrap(), 3.0);
        assert!(eval("1/0").unwrap_err().contains("zero"));
        assert!(eval("2+").is_err());
        assert!(eval("(2").is_err());
        assert!(eval("2)").is_err());
        assert_eq!(show(14.0), "14");
        assert_eq!(show(10.0 / 3.0), "3.333333333333");
        assert_eq!(show(0.5), "0.5");
    }
}
