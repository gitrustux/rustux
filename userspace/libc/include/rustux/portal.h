// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// The file chooser (FoxDen step FD6, docs/FOXDEN.md "The file chooser").
//
// An app in its sandbox has no path to the user's files. It asks: the
// user is shown a file chooser (by appctl, which started the app), picks
// a file or says no, and the app gets a file descriptor for that file
// alone (not its folder, not its path). Without a files: permission this
// is the only way an app reaches the user's files.
//
//   int fd = rx_portal_open("Open a note", "*.txt *.md", name, sizeof name);
//   if (fd >= 0) { read(fd, ...); close(fd); }
//
// Both return the descriptor (rx_portal_open: read only; rx_portal_save:
// write only, the file emptied or made), with the file's name (no folder)
// in name; -ECANCELED when the user said no, -ENOSYS when the program was
// not started by appctl (no chooser to ask), another negative errno when
// the file could not be opened. One question at a time.

#pragma once

#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

// patterns: "*.txt *.md" (what the chooser shows first), or 0 for any file
int rx_portal_open(const char *title, const char *patterns, char *name, size_t cap);
// suggested: the name the chooser offers ("note.txt"), or 0
int rx_portal_save(const char *title, const char *suggested, char *name, size_t cap);

#ifdef __cplusplus
}
#endif
