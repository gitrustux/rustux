// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! rustica-ui: Rustica's toolkit prototype (DESKTOP.md step F5,
//! docs/GUI.md). Our own small toolkit: 2D drawing with tiny-skia, text
//! with fontdue and the Inter font, PNG and JPEG images, and
//! immediate-mode widgets (buttons, text fields, a text editor field,
//! lists, trees, scroll areas, tabs, menus, dialogs) with keyboard
//! navigation, the Dracula theme and integer scaling, in a window of the
//! compositor (rustica-window).
//!
//! ```no_run
//! rustica_ui::run("Hello", 300, 120, None, |ui, ctl| {
//!     ui.label("Hello, Rustica");
//!     if ui.button("Quit") { ctl.quit = true; }
//! }).unwrap();
//! ```

pub mod app;
pub mod gfx;
pub mod ui;

pub use app::{run, Control};
pub use ui::{TreeNode, Ui};
