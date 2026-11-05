// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// Terminal modes. The console service (init) keeps the line editing; a
// program switches between cooked and raw input with tcsetattr. Only
// ICANON, ECHO, ISIG and ICRNL change anything; the other bits are kept.

#ifndef _TERMIOS_H
#define _TERMIOS_H

typedef unsigned char cc_t;
typedef unsigned int speed_t;
typedef unsigned int tcflag_t;

#define NCCS 32
struct termios {
    tcflag_t c_iflag, c_oflag, c_cflag, c_lflag;
    cc_t c_line;
    cc_t c_cc[NCCS];
    speed_t __c_ispeed, __c_ospeed;
};

// c_iflag
#define IGNBRK 0000001
#define BRKINT 0000002
#define IGNPAR 0000004
#define PARMRK 0000010
#define INPCK  0000020
#define ISTRIP 0000040
#define INLCR  0000100
#define IGNCR  0000200
#define ICRNL  0000400
#define IXON   0002000
#define IXOFF  0010000
#define IXANY  0004000
#define IUTF8  0040000
// c_oflag
#define OPOST  0000001
#define ONLCR  0000004
// c_cflag
#define CSIZE  0000060
#define CS8    0000060
#define CREAD  0000200
#define PARENB 0000400
// c_lflag
#define ISIG   0000001
#define ICANON 0000002
#define ECHO   0000010
#define ECHOE  0000020
#define ECHOK  0000040
#define ECHONL 0000100
#define NOFLSH 0000200
#define TOSTOP 0000400
#define IEXTEN 0100000
// c_cc
#define VINTR 0
#define VQUIT 1
#define VERASE 2
#define VKILL 3
#define VEOF 4
#define VTIME 5
#define VMIN 6
#define VSUSP 10
// tcsetattr
#define TCSANOW 0
#define TCSADRAIN 1
#define TCSAFLUSH 2
#define B38400 0000017

int tcgetattr(int fd, struct termios *t);
int tcsetattr(int fd, int when, const struct termios *t);
void cfmakeraw(struct termios *t);
speed_t cfgetospeed(const struct termios *t);
speed_t cfgetispeed(const struct termios *t);
int cfsetospeed(struct termios *t, speed_t s);
int cfsetispeed(struct termios *t, speed_t s);
int tcflush(int fd, int queue);

#endif
