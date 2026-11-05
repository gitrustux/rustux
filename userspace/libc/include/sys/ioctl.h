// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef _SYS_IOCTL_H
#define _SYS_IOCTL_H

struct winsize { unsigned short ws_row, ws_col, ws_xpixel, ws_ypixel; };

#define TCGETS     0x5401
#define TCSETS     0x5402
#define TCSETSW    0x5403
#define TCSETSF    0x5404
#define TIOCGWINSZ 0x5413
#define TIOCSWINSZ 0x5414
#define FIONREAD   0x541B
#define FIONBIO    0x5421
#define FIONCLEX   0x5450
#define FIOCLEX    0x5451

int ioctl(int fd, unsigned long req, ...);

#endif
