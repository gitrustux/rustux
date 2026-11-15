// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! uidemo: the toolkit's demo (DESKTOP.md step F5): a menu bar, tabs with
//! a form, a long list and a tree, a text editor field and images, and
//! dialogs. Prints what is done ("uidemo: ...") for the tests.
//!
//!   uidemo [--scale N] [X,Y]     (or RUSTICA_SCALE=N): 2 for 2x

use rustica_ui::{gfx, Control, TreeNode, Ui};
use std::io::Write;

enum Dialog {
    Thanks(String),
    About,
    Quit,
}

struct Demo {
    tab: usize,
    name: String,
    email: String,
    subscribe: bool,
    items: Vec<String>,
    selected: Option<usize>,
    tree: Vec<TreeNode>,
    tree_sel: String,
    text: String,
    dialog: Option<Dialog>,
    status: String,
    images: Vec<(String, Option<gfx::Pixmap>)>,
}

fn say(s: &str) {
    println!("uidemo: {s}");
    let _ = std::io::stdout().flush();
}

fn tree() -> Vec<TreeNode> {
    vec![
        TreeNode::branch(
            "Rustica",
            vec![
                TreeNode::branch("bin", vec![TreeNode::leaf("compositor"), TreeNode::leaf("uidemo"), TreeNode::leaf("wintest")]),
                TreeNode::branch("etc", vec![TreeNode::leaf("hostname"), TreeNode::leaf("keymap"), TreeNode::branch("svc", vec![TreeNode::leaf("logd.svc"), TreeNode::leaf("authd.svc")])]),
                TreeNode::branch("usr", vec![TreeNode::branch("share", vec![TreeNode::leaf("fonts"), TreeNode::leaf("keymaps"), TreeNode::leaf("man")])]),
            ],
        ),
        TreeNode::branch("Languages", vec![TreeNode::leaf("English"), TreeNode::leaf("Ελληνικά"), TreeNode::leaf("Русский"), TreeNode::leaf("Français")]),
    ]
}

fn frame(d: &mut Demo, ui: &mut Ui, ctl: &mut Control) {
    let menus: [(&str, &[&str]); 3] = [
        ("File", &["New", "About Rustica", "Quit"]),
        ("Edit", &["Clear form", "Fill example"]),
        ("View", &["Form", "List", "Editor", "Images"]),
    ];
    if let Some((m, i)) = ui.menu_bar(&menus) {
        say(&format!("menu {} > {}", menus[m].0, menus[m].1[i]));
        match (m, i) {
            (0, 0) | (1, 0) => {
                d.name.clear();
                d.email.clear();
                d.subscribe = false;
                d.status = "Cleared".into();
            }
            (0, 1) => d.dialog = Some(Dialog::About),
            (0, 2) => d.dialog = Some(Dialog::Quit),
            (1, 1) => {
                d.name = "Ada Lovelace".into();
                d.email = "ada@example.org".into();
            }
            (2, t) => d.tab = t,
            _ => {}
        }
    }
    if ui.tabs("tabs", &["Form", "List", "Editor", "Images"], &mut d.tab) {
        say(&format!("tab {}", d.tab));
    }
    ui.space(6.0);
    match d.tab {
        0 => {
            ui.heading("Sign up");
            ui.dim_label("A form: Tab moves between the fields, Enter submits.");
            ui.space(4.0);
            ui.label("Name");
            let (_, enter1) = ui.text_field("name", &mut d.name, 360.0);
            ui.label("Email");
            let (_, enter2) = ui.text_field("email", &mut d.email, 360.0);
            if ui.checkbox(&mut d.subscribe, "Send me the Rustica newsletter") {
                say(&format!("subscribe {}", d.subscribe));
            }
            ui.space(4.0);
            let (submit, reset) = ui.row(|ui| (ui.primary_button("Submit"), ui.button("Reset")));
            if submit || enter1 || enter2 {
                say(&format!("submit name=\"{}\" email=\"{}\" subscribe={}", d.name, d.email, d.subscribe));
                d.dialog = Some(Dialog::Thanks(d.name.clone()));
            }
            if reset {
                d.name.clear();
                d.email.clear();
                d.subscribe = false;
                say("reset");
            }
        }
        1 => {
            let w = ui.available_width();
            ui.row(|ui| {
                ui.column(w * 0.55, |ui| {
                    ui.label("1000 items (arrows, Page Up/Down, Home, End)");
                    if ui.list("items", &d.items, &mut d.selected, 300.0) {
                        if let Some(s) = d.selected {
                            say(&format!("selected {}", d.items[s]));
                        }
                    }
                });
                ui.column(0.0, |ui| {
                    ui.label("A tree (Right opens, Left closes)");
                    if let Some(p) = ui.tree("tree", &d.tree, 300.0) {
                        d.tree_sel = p.join("/");
                        say(&format!("tree {}", d.tree_sel));
                    }
                });
            });
        }
        2 => {
            ui.label("A text editor field: selection with Shift or the mouse, Ctrl+C, Ctrl+X, Ctrl+V, Ctrl+A");
            if ui.text_area("editor", &mut d.text, 0.0, 330.0) {
                d.status = format!("{} characters, {} lines", d.text.chars().count(), d.text.lines().count());
                say(&format!("editor {:?}", d.text.chars().take(40).collect::<String>()));
            }
        }
        _ => {
            ui.label("PNG and JPEG, decoded by the toolkit");
            ui.scroll_area("images", 330.0, |ui| {
                for (name, img) in &d.images {
                    ui.dim_label(name);
                    match img {
                        Some(p) => ui.image(p),
                        None => ui.label("(could not be read)"),
                    }
                }
            });
        }
    }
    // The status line at the bottom
    ui.space(4.0);
    ui.separator();
    let st = if d.status.is_empty() { "Ready".to_string() } else { d.status.clone() };
    ui.dim_label(&st);

    match &d.dialog {
        Some(Dialog::Thanks(n)) => {
            let msg = format!("Thank you, {}!", if n.is_empty() { "whoever you are" } else { n });
            if let Some(b) = ui.dialog("Submitted", &msg, &["OK"]) {
                say(&format!("dialog Submitted: {}", ["OK"][b]));
                d.status = format!("Submitted {}", d.name);
                d.dialog = None;
            }
        }
        Some(Dialog::About) => {
            if let Some(_) = ui.dialog("About", "Rustica's toolkit prototype: tiny-skia, fontdue, Inter.", &["Close"]) {
                say("dialog About: Close");
                d.dialog = None;
            }
        }
        Some(Dialog::Quit) => {
            if let Some(b) = ui.dialog("Quit", "Close the demo?", &["Quit", "Cancel"]) {
                say(&format!("dialog Quit: {}", ["Quit", "Cancel"][b]));
                d.dialog = None;
                if b == 0 {
                    ctl.quit = true;
                }
            }
        }
        None => {}
    }
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() >= 2 && args[0] == "--scale" {
        std::env::set_var("RUSTICA_SCALE", &args[1]);
        args.drain(..2);
    }
    let at = args.first().and_then(|a| {
        let (x, y) = a.split_once(',')?;
        Some((x.parse().ok()?, y.parse().ok()?))
    });
    let images = ["/usr/share/rustica/logo.png", "/usr/share/rustica/photo.jpg"]
        .iter()
        .map(|p| (p.to_string(), gfx::load_image(p).ok()))
        .collect();
    let mut d = Demo {
        tab: 0,
        name: String::new(),
        email: String::new(),
        subscribe: false,
        items: (1..=1000).map(|i| format!("Item {i:04}")).collect(),
        selected: None,
        tree: tree(),
        tree_sel: String::new(),
        text: "Rustica's text editor field.\n\nLatin: The quick brown fox jumps over the lazy dog.\nΕλληνικά: Ξεσκεπάζω την ψυχοφθόρα βδελυγμία.\nКириллица: Съешь же ещё этих мягких французских булок.\n\nEdit me.".into(),
        dialog: None,
        status: String::new(),
        images,
    };
    say("started");
    let mut first = true;
    let r = rustica_ui::run("Rustica UI demo", 760, 520, at, |ui, ctl| {
        if first {
            ui.set_focus("name");
            first = false;
        }
        frame(&mut d, ui, ctl);
    });
    match r {
        Ok(()) => say("closed"),
        Err(e) => {
            eprintln!("uidemo: {e}");
            std::process::exit(1);
        }
    }
}
