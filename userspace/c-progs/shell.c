// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Rustux Shell
//!
//! Reads a line from the console and runs it. Supported (steps 7 and 8):
//!
//! - quoting: '...' (literal), "..." ($ expanded; \" \\ \$ escaped) and \x
//! - variables: $NAME, ${NAME}, $? (exit code of the last command);
//!   NAME=value sets a shell variable, `export` puts it in the environment
//!   of the programs started from here; NAME=value before a command sets it
//!   for that command only
//! - pipelines `a | b | c`, redirection `<`, `>`, `>>`, `2>`, `2>>`, `2>&1`
//! - lists: `a ; b`, `a && b` (b if a succeeded), `a || b` (b if a failed);
//!   each part is expanded just before it runs
//! - built-ins: cd, pwd, export, unset, set, help, clear, loadkeys,
//!   restart-kbd, exit
//! - programs are looked up in the directories of $PATH (/bin)
//! - line editing, history and Tab completion (lineedit.h; step 10): the
//!   console is in raw mode while a line is typed and back in cooked mode
//!   while programs run
//! - Ctrl+C ends the programs in the foreground; Ctrl+D at the prompt is
//!   ignored (type `exit`)

#include <ctype.h>
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <sys/stat.h>
#include <rustux/console.h>
#include <rustux/process.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <dirent.h>

#define LINE_MAX_LEN 4096
#define MAX_WORDS 128

#define ANSI_RESET   "\033[0m"
#define ANSI_RED     "\033[31m"
#define ANSI_GREEN   "\033[32m"
#define ANSI_MAGENTA "\033[35m"
#define ANSI_CYAN    "\033[36m"

#include "lineedit.h"

static rx_handle_t init_ctl; // control channel to init (restart-kbd)
static rx_handle_t console;  // our console handle (for Ctrl+C and draining)
static rx_handle_t power;    // power resource, passed on to programs (reboot, poweroff)
static int last_status;

// ---- shell variables (not exported) ----

struct var { char *name, *value; };
static struct var vars[64];
static int nvars;

static struct var *find_var(const char *name) {
    for (int i = 0; i < nvars; i++) if (!strcmp(vars[i].name, name)) return &vars[i];
    return 0;
}

static void drop_var(const char *name) {
    struct var *v = find_var(name);
    if (!v) return;
    free(v->name);
    free(v->value);
    *v = vars[--nvars];
}

static const char *get_var(const char *name) {
    struct var *v = find_var(name);
    return v ? v->value : getenv(name);
}

// NAME=value: the environment if NAME is exported, else a shell variable
static void set_var(const char *name, const char *value) {
    if (getenv(name)) {
        setenv(name, value, 1);
        return;
    }
    struct var *v = find_var(name);
    if (v) {
        free(v->value);
        v->value = strdup(value);
    } else if (nvars < (int)(sizeof vars / sizeof *vars)) {
        vars[nvars].name = strdup(name);
        vars[nvars++].value = strdup(value);
    } else {
        fprintf(stderr, "shell: too many variables\n");
    }
}

static int is_name(const char *s, size_t n) {
    if (!n || !(isalpha((unsigned char)s[0]) || s[0] == '_')) return 0;
    for (size_t i = 1; i < n; i++) if (!(isalnum((unsigned char)s[i]) || s[i] == '_')) return 0;
    return 1;
}

// ---- parsing ----

struct word {
    char *text;
    int assignment; // NAME=... with NAME and '=' written literally
};

struct buf { char *p; size_t len, cap; };

static void put(struct buf *b, const char *s, size_t n) {
    if (b->len + n + 1 > b->cap) {
        b->cap = (b->len + n + 1) * 2;
        b->p = realloc(b->p, b->cap);
    }
    memcpy(b->p + b->len, s, n);
    b->len += n;
    b->p[b->len] = 0;
}

// Expand the variable after '$' at *s; advances *s
static void expand(struct buf *b, const char **s) {
    const char *p = *s;
    char num[24];
    if (*p == '?') {
        int n = snprintf(num, sizeof num, "%d", last_status);
        put(b, num, (size_t)n);
        *s = p + 1;
        return;
    }
    int braced = *p == '{';
    if (braced) p++;
    const char *start = p;
    while (isalnum((unsigned char)*p) || *p == '_') p++;
    size_t n = (size_t)(p - start);
    if (!n || (braced && *p != '}')) {
        put(b, "$", 1); // not a variable: keep the '$'
        return;
    }
    char name[128];
    if (n >= sizeof name) n = sizeof name - 1;
    memcpy(name, start, n);
    name[n] = 0;
    const char *v = get_var(name);
    if (v) put(b, v, strlen(v));
    *s = braced ? p + 1 : p;
}

enum redir_kind { R_IN, R_OUT, R_APPEND, R_ERR, R_ERR_APPEND, R_ERR_TO_OUT };

struct redir {
    enum redir_kind kind;
    char *path; // NULL for 2>&1
};

#define MAX_REDIRS 8
#define MAX_STAGES 16

struct stage {
    struct word w[MAX_WORDS];
    int n;
    struct redir r[MAX_REDIRS];
    int nr;
};

struct pipeline {
    struct stage st[MAX_STAGES];
    int n;
};

// What ends a pipeline
enum { END_LINE, END_SEMI, END_AND, END_OR };

static void free_pipeline(struct pipeline *pl) {
    for (int i = 0; i < pl->n; i++) {
        for (int j = 0; j < pl->st[i].n; j++) free(pl->st[i].w[j].text);
        for (int j = 0; j < pl->st[i].nr; j++) free(pl->st[i].r[j].path);
    }
    pl->n = 0;
}

// Read one word at *s (expanded); returns NULL after printing an error
static char *read_word(const char **ps, int *assignment) {
    const char *s = *ps;
    struct buf b = { 0, 0, 0 };
    put(&b, "", 0);
    int candidate = 1;
    *assignment = 0;
    while (*s && !strchr(" \t\n|&;<>()`", *s)) {
        char c = *s;
        if (c == '\'') {
            const char *end = strchr(s + 1, '\'');
            if (!end) { fprintf(stderr, "shell: missing closing '\n"); free(b.p); return 0; }
            put(&b, s + 1, (size_t)(end - s - 1));
            s = end + 1;
            candidate = 0;
        } else if (c == '"') {
            s++;
            while (*s && *s != '"') {
                if (*s == '\\' && (s[1] == '"' || s[1] == '\\' || s[1] == '$')) {
                    put(&b, s + 1, 1);
                    s += 2;
                } else if (*s == '$') {
                    s++;
                    expand(&b, &s);
                } else {
                    put(&b, s++, 1);
                }
            }
            if (*s != '"') { fprintf(stderr, "shell: missing closing \"\n"); free(b.p); return 0; }
            s++;
            candidate = 0;
        } else if (c == '\\') {
            if (s[1] && s[1] != '\n') put(&b, s + 1, 1);
            s += s[1] ? 2 : 1;
            candidate = 0;
        } else if (c == '$') {
            s++;
            expand(&b, &s);
            candidate = 0;
        } else {
            if (c == '=' && candidate && !*assignment && is_name(b.p, b.len)) *assignment = 1;
            put(&b, s++, 1);
        }
    }
    *ps = s;
    return b.p;
}

// Parse one pipeline from *ps; sets *end to what ended it. Returns 0, or
// -1 after printing an error (the rest of the line is skipped).
static int parse_pipeline(const char **ps, struct pipeline *pl, int *end) {
    const char *s = *ps;
    pl->n = 1;
    struct stage *st = &pl->st[0];
    st->n = st->nr = 0;
    *end = END_LINE;
    for (;;) {
        while (*s == ' ' || *s == '\t' || *s == '\n') s++;
        if (!*s || *s == '#') break;
        if (*s == ';') { s++; *end = END_SEMI; break; }
        if (s[0] == '&' && s[1] == '&') { s += 2; *end = END_AND; break; }
        if (s[0] == '|' && s[1] == '|') { s += 2; *end = END_OR; break; }
        if (*s == '|') {
            s++;
            if (st->n == 0) { fprintf(stderr, "shell: missing command before '|'\n"); goto bad; }
            if (pl->n == MAX_STAGES) { fprintf(stderr, "shell: pipeline too long\n"); goto bad; }
            st = &pl->st[pl->n++];
            st->n = st->nr = 0;
            continue;
        }
        if (strchr("&()`", *s)) {
            fprintf(stderr, "shell: '%c' is not supported yet\n", *s);
            goto bad;
        }
        // Redirections: < > >> 2> 2>> 2>&1
        int kind = -1;
        if (*s == '<') { kind = R_IN; s++; }
        else if (s[0] == '>' && s[1] == '>') { kind = R_APPEND; s += 2; }
        else if (*s == '>') { kind = R_OUT; s++; }
        else if (s[0] == '2' && s[1] == '>' && s[2] == '&' && s[3] == '1') { kind = R_ERR_TO_OUT; s += 4; }
        else if (s[0] == '2' && s[1] == '>' && s[2] == '>') { kind = R_ERR_APPEND; s += 3; }
        else if (s[0] == '2' && s[1] == '>') { kind = R_ERR; s += 2; }
        if (kind >= 0) {
            if (st->nr == MAX_REDIRS) { fprintf(stderr, "shell: too many redirections\n"); goto bad; }
            char *path = 0;
            if (kind != R_ERR_TO_OUT) {
                while (*s == ' ' || *s == '\t') s++;
                int dummy;
                if (!*s || strchr("|&;<>()`\n", *s)) { fprintf(stderr, "shell: missing file name after redirection\n"); goto bad; }
                path = read_word(&s, &dummy);
                if (!path) goto bad;
            }
            st->r[st->nr].kind = (enum redir_kind)kind;
            st->r[st->nr++].path = path;
            continue;
        }
        if (st->n == MAX_WORDS) { fprintf(stderr, "shell: too many words\n"); goto bad; }
        int assignment;
        char *w = read_word(&s, &assignment);
        if (!w) goto bad;
        st->w[st->n].text = w;
        st->w[st->n++].assignment = assignment;
    }
    *ps = s;
    if (pl->n > 1 && (st->n == 0)) { fprintf(stderr, "shell: missing command after '|'\n"); free_pipeline(pl); return -1; }
    return 0;
bad:
    free_pipeline(pl);
    return -1;
}

// ---- running programs ----

// Discard typed input nobody read (after Ctrl+C, like a Unix terminal)
static void drain_input(void) {
    char buf[4 + RX_CONSOLE_MAX_DATA];
    uint32_t n;
    while (rx_channel_read(console, 0, buf, 0, sizeof buf, 0, &n, 0) == RX_OK) {}
}

static void clear_interrupt(void) {
    rx_object_signal(console, RX_CONSOLE_SIGNAL_INTERRUPT, 0);
}

static void report(int64_t code) {
    if (code < 0) {
        const char *why = code == RX_TASK_RETCODE_SYSCALL_KILL ? "killed"
                        : code == RX_TASK_RETCODE_EXCEPTION_KILL ? "crashed (CPU exception)" : "exit code";
        printf(ANSI_RED "[%s: %lld]" ANSI_RESET "\n", why, (long long)code);
    }
}

// Wait for the processes of a pipeline (all in `job`), ending them all on
// Ctrl+C. Returns the exit code of the last one.
static int wait_foreground(rx_handle_t job, rx_handle_t *procs, int n, int last) {
    int64_t codes[MAX_STAGES];
    int left = 0;
    for (int i = 0; i < n; i++) {
        codes[i] = 127; // never started
        if (procs[i]) left++;
    }
    while (left > 0) {
        rx_wait_item_t items[MAX_STAGES + 1];
        int map[MAX_STAGES], m = 0;
        for (int i = 0; i < n; i++) {
            if (!procs[i]) continue;
            items[m] = (rx_wait_item_t){ procs[i], RX_PROCESS_TERMINATED, 0 };
            map[m++] = i;
        }
        items[m] = (rx_wait_item_t){ console, RX_CONSOLE_SIGNAL_INTERRUPT, 0 };
        rx_object_wait_many(items, (uint64_t)m + (console ? 1 : 0), RX_TIME_INFINITE);
        if (console && (items[m].pending & RX_CONSOLE_SIGNAL_INTERRUPT)) {
            rx_task_kill(job);
            for (int i = 0; i < n; i++) {
                if (procs[i]) rx_wait_process(procs[i]);
                procs[i] = 0;
            }
            clear_interrupt();
            drain_input();
            return 130; // as for SIGINT in a Unix shell
        }
        for (int k = 0; k < m; k++) {
            if (items[k].pending & RX_PROCESS_TERMINATED) {
                int i = map[k];
                codes[i] = rx_wait_process(procs[i]);
                procs[i] = 0;
                left--;
            }
        }
    }
    report(codes[last]);
    return (int)codes[last];
}

// Environment for a program: ours, with the command's NAME=value words
static char **child_env(struct word *w, int nassign) {
    extern char **environ;
    int n = 0;
    while (environ[n]) n++;
    char **env = malloc((size_t)(n + nassign + 1) * sizeof *env);
    int k = 0;
    for (int i = 0; i < n; i++) {
        size_t len = strcspn(environ[i], "=");
        int overridden = 0;
        for (int j = 0; j < nassign; j++) {
            if (!strncmp(w[j].text, environ[i], len) && w[j].text[len] == '=') overridden = 1;
        }
        if (!overridden) env[k++] = environ[i];
    }
    for (int j = 0; j < nassign; j++) env[k++] = w[j].text;
    env[k] = 0;
    return env;
}

// Find a program: as given if it has a '/', else in $PATH. Returns 0, or
// 127 (not found) / 126 (not runnable) after printing why.
static int find_program(const char *name, char *path, size_t cap) {
    struct stat st;
    if (strchr(name, '/')) {
        snprintf(path, cap, "%s", name);
        if (stat(path, &st) < 0) {
            fprintf(stderr, "shell: %s: %s\n", name, strerror(errno));
            return 127;
        }
    } else {
        const char *dirs = get_var("PATH");
        if (!dirs) dirs = "/bin";
        int found = 0;
        while (*dirs && !found) {
            size_t len = strcspn(dirs, ":");
            snprintf(path, cap, "%.*s/%s", (int)len, dirs, name);
            found = stat(path, &st) == 0 && !S_ISDIR(st.st_mode);
            dirs += len + (dirs[len] == ':');
        }
        if (!found) {
            fprintf(stderr, ANSI_RED "shell: command not found: " ANSI_RESET "%s\n", name);
            return 127;
        }
    }
    if (S_ISDIR(st.st_mode)) {
        fprintf(stderr, "shell: %s: is a directory\n", name);
        return 126;
    }
    return 0;
}

// Apply a stage's redirections to our fds 0-2. Returns 0, or 1 after
// printing why one failed.
static int apply_redirs(struct stage *st) {
    for (int i = 0; i < st->nr; i++) {
        struct redir *r = &st->r[i];
        if (r->kind == R_ERR_TO_OUT) {
            dup2(1, 2);
            continue;
        }
        int flags = r->kind == R_IN ? O_RDONLY
                  : (r->kind == R_APPEND || r->kind == R_ERR_APPEND) ? O_WRONLY | O_CREAT | O_APPEND
                  : O_WRONLY | O_CREAT | O_TRUNC;
        int fd = open(r->path, flags, 0644);
        if (fd < 0) {
            fprintf(stderr, "shell: %s: %s\n", r->path, strerror(errno));
            return 1;
        }
        dup2(fd, r->kind == R_IN ? 0 : (r->kind == R_ERR || r->kind == R_ERR_APPEND) ? 2 : 1);
        close(fd);
    }
    return 0;
}

static int builtin(int argc, char **argv, int *status);

static const char *const builtin_names[] = { "help", "clear", "export", "unset", "set", "restart-kbd",
                                             "loadkeys", "exit", "cd", "pwd", 0 };
static const char *const *le_builtins(void) { return builtin_names; }

static int is_builtin(const char *c) {
    const char *const *names = builtin_names;
    for (int i = 0; names[i]; i++) if (!strcmp(c, names[i])) return 1;
    return 0;
}

static int run_pipeline(struct pipeline *pl) {
    // Our own stdin/stdout/stderr, saved while the stages' fds are set up
    int saved[3];
    for (int i = 0; i < 3; i++) saved[i] = dup(i);
    rx_handle_t job = 0, procs[MAX_STAGES] = { 0 };
    int status = 0, prev_read = -1;

    // Only assignments: set shell variables
    if (pl->n == 1 && pl->st[0].n > 0) {
        struct stage *st = &pl->st[0];
        int all = 1;
        for (int i = 0; i < st->n; i++) all &= st->w[i].assignment;
        if (all && st->nr == 0) {
            for (int i = 0; i < st->n; i++) {
                char *eq = strchr(st->w[i].text, '=');
                *eq = 0;
                set_var(st->w[i].text, eq + 1);
                *eq = '=';
            }
            status = 0;
            goto done;
        }
    }

    if (rx_job_create(rx_job_default, 0, &job) != RX_OK) job = 0;
    for (int i = 0; i < pl->n; i++) {
        struct stage *st = &pl->st[i];
        int nassign = 0;
        while (nassign < st->n && st->w[nassign].assignment) nassign++;
        char *argv[MAX_WORDS + 1];
        int argc = 0;
        for (int k = nassign; k < st->n; k++) argv[argc++] = st->w[k].text;
        argv[argc] = 0;

        // fds for this stage: pipe from the previous one, pipe to the next
        for (int k = 0; k < 3; k++) dup2(saved[k], k);
        if (prev_read >= 0) {
            dup2(prev_read, 0);
            close(prev_read);
            prev_read = -1;
        }
        int p[2] = { -1, -1 };
        if (i + 1 < pl->n && pipe(p) == 0) {
            dup2(p[1], 1);
            close(p[1]);
            prev_read = p[0];
        }
        int st_status = apply_redirs(st);
        if (st_status == 0 && argc == 0) st_status = 0; // only redirections (creates files)
        else if (st_status == 0 && is_builtin(argv[0])) {
            if (pl->n > 1) {
                fprintf(stderr, "shell: built-in '%s' can't be used in a pipeline yet\n", argv[0]);
                st_status = 1;
            } else {
                builtin(argc, argv, &st_status);
                fflush(stdout);
                fflush(stderr);
            }
        } else if (st_status == 0) {
            char path[PATH_MAX];
            st_status = find_program(argv[0], path, sizeof path);
            if (st_status == 0) {
                char **env = child_env(st->w, nassign);
                // The power resource and the system job (for reboot,
                // poweroff, ps and kill)
                rx_spawn_handle_t extra[2];
                uint32_t nextra = 0;
                if (power && rx_handle_duplicate(power, RX_RIGHT_SAME_RIGHTS, &extra[nextra].handle) == RX_OK)
                    extra[nextra++].info = RX_PA_POWER_RESOURCE;
                if (rx_handle_duplicate(rx_job_default, RX_RIGHT_SAME_RIGHTS, &extra[nextra].handle) == RX_OK)
                    extra[nextra++].info = RX_PA_JOB_SYSTEM;
                rx_status_t r = rx_spawn_etc(job ? job : rx_job_default, path, argv, env, RX_SPAWN_CLONE_ALL, extra, nextra, &procs[i]);
                free(env);
                if (r != RX_OK) {
                    fprintf(stderr, "shell: cannot start %s: %s\n", argv[0], strerror(errno));
                    st_status = 126;
                    procs[i] = 0;
                }
            }
        }
        if (i == pl->n - 1) status = st_status;
    }
    if (prev_read >= 0) close(prev_read);
    for (int k = 0; k < 3; k++) dup2(saved[k], k);

    int any = 0, last_ran = procs[pl->n - 1] != 0;
    for (int i = 0; i < pl->n; i++) any |= procs[i] != 0;
    if (any) {
        int code = wait_foreground(job, procs, pl->n, pl->n - 1);
        if (last_ran || code == 130) status = code;
    }
done:
    if (job) rx_handle_close(job);
    for (int i = 0; i < 3; i++) close(saved[i]);
    return status;
}

// ---- built-in commands ----

static void cmd_help(void) {
    printf("\n" ANSI_CYAN "Built-in commands:" ANSI_RESET "\n"
           "  help                 this list\n"
           "  cd [DIR]             change directory ($HOME without DIR, - for the last one)\n"
           "  pwd                  print the current directory\n"
           "  export NAME[=VALUE]  put a variable in the environment of programs\n"
           "  unset NAME           remove a variable\n"
           "  set                  list all variables\n"
           "  clear                clear the screen\n"
           "  loadkeys [LAYOUT]    keyboard layout (us, de, ...; none: list them)\n"
           "  restart-kbd          kill the keyboard driver (init starts a new one)\n"
           "  exit [CODE]          leave the shell (init starts a new one)\n"
           "\n" ANSI_CYAN "Programs in /bin:" ANSI_RESET "\n"
           "  ls, cat, echo, mkdir, rm, mv, upper, env, true, false, hello, counter, args\n"
           "  ps, kill, uptime, free, date, uname, hostname, dmesg, reboot, poweroff\n"
           "  termtest (a full-screen program), hello-std (Rust with std)\n"
           "  tests: systest, fstest, libctest, threadtest, objtest, ipctest, spawntest, drvtest,\n"
           "         smptest, spin\n"
           "\n" ANSI_CYAN "Shell:" ANSI_RESET "\n"
           "  a | b, < file, > file, >> file, 2> file, 2>&1, a ; b, a && b, a || b\n"
           "  'single' and \"double\" quotes, \\ escapes, $NAME, ${NAME}, $? (last exit code),\n"
           "  NAME=value, NAME=value command. Ctrl+C stops a program.\n"
           "  Editing: arrows, Home/End, Ctrl+A/E/K/U/W/L, Up/Down for history, Tab completes.\n"
           "  Shift+PgUp/PgDn scroll back through earlier output.\n"
           "  Files: / and /tmp are in memory (lost at reboot); /bin and /boot are read-only.\n\n");
}

static int cmd_export(int argc, char **argv) {
    extern char **environ;
    if (argc == 1) {
        for (char **e = environ; *e; e++) printf("export %s\n", *e);
        return 0;
    }
    int status = 0;
    for (int i = 1; i < argc; i++) {
        char *eq = strchr(argv[i], '=');
        size_t n = eq ? (size_t)(eq - argv[i]) : strlen(argv[i]);
        if (!is_name(argv[i], n)) {
            fprintf(stderr, "export: not a valid name: %s\n", argv[i]);
            status = 1;
            continue;
        }
        char name[128];
        snprintf(name, sizeof name, "%.*s", (int)n, argv[i]);
        const char *value = eq ? eq + 1 : (find_var(name) ? find_var(name)->value : getenv(name));
        setenv(name, value ? value : "", 1);
        drop_var(name);
    }
    return status;
}

static int cmd_set(void) {
    extern char **environ;
    for (char **e = environ; *e; e++) printf("%s\n", *e);
    for (int i = 0; i < nvars; i++) printf("%s=%s\n", vars[i].name, vars[i].value);
    return 0;
}

// loadkeys [LAYOUT]: choose the keyboard layout (/etc/keymap) and have
// init load it; without LAYOUT, show the current one and the choices
static int cmd_loadkeys(int argc, char **argv) {
    char cur[32] = "us";
    int fd = open("/etc/keymap", O_RDONLY);
    if (fd >= 0) {
        ssize_t n = read(fd, cur, sizeof cur - 1);
        close(fd);
        cur[n > 0 ? n : 0] = 0;
        cur[strcspn(cur, " \t\r\n")] = 0;
        if (!cur[0]) strcpy(cur, "us");
    }
    if (argc < 2) {
        printf("layout: %s\navailable: us", cur);
        DIR *d = opendir("/usr/share/keymaps");
        struct dirent *de;
        while (d && (de = readdir(d))) printf(" %s", de->d_name);
        if (d) closedir(d);
        printf("\n");
        return 0;
    }
    const char *name = argv[1];
    char path[128];
    snprintf(path, sizeof path, "/usr/share/keymaps/%s", name);
    if (strcmp(name, "us") && (strchr(name, '/') || access(path, R_OK) < 0)) {
        fprintf(stderr, "loadkeys: no layout named %s (see loadkeys without arguments)\n", name);
        return 1;
    }
    fd = open("/etc/keymap", O_WRONLY | O_CREAT | O_TRUNC, 0644);
    if (fd < 0 || write(fd, name, strlen(name)) < 0 || write(fd, "\n", 1) < 0) {
        fprintf(stderr, "loadkeys: /etc/keymap: %s\n", strerror(errno));
        if (fd >= 0) close(fd);
        return 1;
    }
    close(fd);
    if (!init_ctl || rx_channel_write(init_ctl, 0, "reload-keymap", 13, 0, 0) != RX_OK) {
        fprintf(stderr, "loadkeys: init is not listening\n");
        return 1;
    }
    printf("keyboard layout: %s\n", name);
    return 0;
}

// Returns 1 and sets *status if argv[0] is a built-in
static int builtin(int argc, char **argv, int *status) {
    const char *c = argv[0];
    *status = 0;
    if (!strcmp(c, "help")) cmd_help();
    else if (!strcmp(c, "clear")) printf("\033[2J\033[H");
    else if (!strcmp(c, "export")) *status = cmd_export(argc, argv);
    else if (!strcmp(c, "unset")) {
        for (int i = 1; i < argc; i++) { drop_var(argv[i]); unsetenv(argv[i]); }
    } else if (!strcmp(c, "set")) *status = cmd_set();
    else if (!strcmp(c, "restart-kbd")) {
        if (!init_ctl || rx_channel_write(init_ctl, 0, "restart-kbd", 11, 0, 0) != RX_OK) {
            fprintf(stderr, "restart-kbd: init is not listening\n");
            *status = 1;
        }
    } else if (!strcmp(c, "loadkeys")) {
        *status = cmd_loadkeys(argc, argv);
    } else if (!strcmp(c, "cd")) {
        const char *dir = argc > 1 ? argv[1] : get_var("HOME");
        char old[PATH_MAX];
        getcwd(old, sizeof old);
        if (dir && !strcmp(dir, "-")) {
            dir = get_var("OLDPWD");
            if (dir) printf("%s\n", dir);
        }
        if (!dir) {
            fprintf(stderr, "cd: no directory\n");
            *status = 1;
        } else if (chdir(dir) < 0) {
            fprintf(stderr, "cd: %s: %s\n", dir, strerror(errno));
            *status = 1;
        } else {
            setenv("OLDPWD", old, 1);
        }
    } else if (!strcmp(c, "pwd")) {
        char cwd[PATH_MAX];
        printf("%s\n", getcwd(cwd, sizeof cwd) ? cwd : "?");
    } else if (!strcmp(c, "exit")) {
        int code = argc > 1 ? atoi(argv[1]) : last_status;
        printf(ANSI_GREEN "Exiting shell." ANSI_RESET "\n");
        exit(code);
    } else return 0;
    return 1;
}

static void execute(const char *line) {
    static struct pipeline pl;
    const char *s = line;
    for (;;) {
        int end;
        while (*s == ' ' || *s == '\t' || *s == '\n') s++;
        if (!*s || *s == '#') return;
        if (parse_pipeline(&s, &pl, &end) < 0) {
            last_status = 2;
            return;
        }
        if (pl.st[0].n == 0 && pl.st[0].nr == 0 && pl.n == 1) {
            if (end != END_LINE) {
                fprintf(stderr, "shell: missing command before a list operator\n");
                last_status = 2;
            }
            return;
        }
        last_status = run_pipeline(&pl);
        free_pipeline(&pl);
        if (last_status == 130 || end == END_LINE) return; // Ctrl+C stops the whole line
        // && and ||: skip the next pipelines while the condition fails
        while ((end == END_AND && last_status != 0) || (end == END_OR && last_status == 0)) {
            if (parse_pipeline(&s, &pl, &end) < 0) return; // (expanded but not run)
            free_pipeline(&pl);
            if (end == END_LINE) return;
        }
    }
}

// ---- input ----

// Read one line from the console. Returns its length, 0 at end of input
// (Ctrl+D on an empty line), -1 if Ctrl+C was pressed.
static int read_line(char *buf, size_t cap) {
    rx_handle_t in = console;
    size_t len = 0;
    for (;;) {
        uint32_t observed = 0;
        rx_object_wait_one(in, RX_CHANNEL_READABLE | RX_PEER_CLOSED | RX_CONSOLE_SIGNAL_INTERRUPT,
                           RX_TIME_INFINITE, &observed);
        if (observed & RX_CONSOLE_SIGNAL_INTERRUPT) {
            clear_interrupt();
            return -1;
        }
        ssize_t n = read(0, buf + len, cap - len - 1);
        if (n <= 0) {
            buf[len] = 0;
            return (int)len;
        }
        len += (size_t)n;
        buf[len] = 0;
        if (buf[len - 1] == '\n' || len == cap - 1) return (int)len;
    }
}

static void show_welcome(void) {
    printf("\n" ANSI_MAGENTA
           "╔════════════════════════════════════════════════════════════════╗\n"
           "║                                                                ║\n"
           "║                    Welcome to Rustux OS                        ║\n"
           "║                                                                ║\n"
           "║  Type 'help' for available commands                            ║\n"
           "║                                                                ║\n"
           "╚════════════════════════════════════════════════════════════════╝\n"
           ANSI_RESET "\n");
}

int main(void) {
    static char line[LINE_MAX_LEN];
    static struct le editor;
    init_ctl = rx_take_startup_handle(RX_PA_USER0);
    power = rx_take_startup_handle(RX_PA_POWER_RESOURCE);
    if (!rx_fd_handle(0) || rx_handle_duplicate(rx_fd_handle(0), RX_RIGHT_SAME_RIGHTS, &console) != RX_OK) {
        fprintf(stderr, "shell: no console\n");
        return 1;
    }
    show_welcome();
    editor.console = console;
    for (;;) {
        char cwd[PATH_MAX], prompt[PATH_MAX + 64];
        if (!getcwd(cwd, sizeof cwd)) strcpy(cwd, "?");
        snprintf(prompt, sizeof prompt, ANSI_MAGENTA "rustux" ANSI_RESET ":" ANSI_CYAN "%s" ANSI_RESET " > ", cwd);
        fflush(stdout);
        int n;
        if (le_raw()) {
            n = le_read(&editor, prompt, line, sizeof line);
            le_cooked_mode();
        } else {
            fputs(prompt, stdout); // not a console: no editing
            fflush(stdout);
            n = read_line(line, sizeof line);
        }
        if (n < 0) continue; // Ctrl+C: new prompt
        if (n == 0) {
            if (!(rx_object_wait_one(console, RX_PEER_CLOSED, 0, 0) == RX_OK)) {
                printf("\nUse \"exit\" to leave the shell.\n");
                continue;
            }
            return 0; // the console is gone
        }
        execute(line);
    }
}
