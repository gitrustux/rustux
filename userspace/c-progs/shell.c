// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Rustux Shell
//!
//! Reads a line from the console, splits it into words and runs the
//! command. Supported so far (step 7):
//!
//! - quoting: '...' (literal), "..." ($ expanded; \" \\ \$ escaped) and \x
//! - variables: $NAME, ${NAME}, $? (exit code of the last command);
//!   NAME=value sets a shell variable, `export` puts it in the environment
//!   of the programs started from here; NAME=value before a command sets it
//!   for that command only
//! - programs are looked up in the directories of $PATH (/bin)
//! - Ctrl+C ends the program in the foreground; Ctrl+D at the prompt is
//!   ignored (type `exit`)
//!
//! Pipes, redirection, `;`, `&&`, `||`, `cd` and `pwd` come with the
//! filesystem in step 8.

#include <ctype.h>
#include <rustux/console.h>
#include <rustux/process.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#define LINE_MAX_LEN 4096
#define MAX_WORDS 128

#define ANSI_RESET   "\033[0m"
#define ANSI_RED     "\033[31m"
#define ANSI_GREEN   "\033[32m"
#define ANSI_MAGENTA "\033[35m"
#define ANSI_CYAN    "\033[36m"

static rx_handle_t init_ctl; // control channel to init (restart-kbd)
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

// Split `line` into words. Returns the count, or -1 after printing an error.
static int parse(const char *line, struct word *words, int max) {
    int n = 0;
    const char *s = line;
    for (;;) {
        while (*s == ' ' || *s == '\t' || *s == '\n') s++;
        if (!*s || *s == '#') return n;
        if (n == max) {
            fprintf(stderr, "shell: too many words\n");
            return -1;
        }
        struct buf b = { 0, 0, 0 };
        put(&b, "", 0);
        int candidate = 1, assignment = 0; // could still be NAME=
        while (*s && *s != ' ' && *s != '\t' && *s != '\n') {
            char c = *s;
            if (c == '\'') {
                const char *end = strchr(s + 1, '\'');
                if (!end) { fprintf(stderr, "shell: missing closing '\n"); free(b.p); return -1; }
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
                if (*s != '"') { fprintf(stderr, "shell: missing closing \"\n"); free(b.p); return -1; }
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
            } else if (strchr("|<>;&()`", c)) {
                fprintf(stderr, "shell: '%c' is not supported yet (pipes, redirection and lists come in step 8)\n", c);
                free(b.p);
                return -1;
            } else {
                if (c == '=' && candidate && !assignment && is_name(b.p, b.len)) assignment = 1;
                put(&b, s++, 1);
            }
        }
        words[n].text = b.p;
        words[n++].assignment = assignment;
    }
}

// ---- running programs ----

// Discard typed input nobody read (after Ctrl+C, like a Unix terminal)
static void drain_input(void) {
    char buf[4 + RX_CONSOLE_MAX_DATA];
    uint32_t n;
    while (rx_channel_read(rx_fd_handle(0), 0, buf, 0, sizeof buf, 0, &n, 0) == RX_OK) {}
}

static void clear_interrupt(void) {
    rx_object_signal(rx_fd_handle(0), RX_CONSOLE_SIGNAL_INTERRUPT, 0);
}

// Wait for `proc`, ending it on Ctrl+C. Returns its exit code.
static int wait_foreground(rx_handle_t proc) {
    rx_handle_t in = rx_fd_handle(0);
    for (;;) {
        rx_wait_item_t items[2] = {
            { proc, RX_PROCESS_TERMINATED, 0 },
            { in, RX_CONSOLE_SIGNAL_INTERRUPT, 0 },
        };
        rx_object_wait_many(items, in ? 2 : 1, RX_TIME_INFINITE);
        if (items[0].pending & RX_PROCESS_TERMINATED) break;
        if (items[1].pending & RX_CONSOLE_SIGNAL_INTERRUPT) {
            rx_task_kill(proc);
            rx_object_wait_one(proc, RX_PROCESS_TERMINATED, RX_TIME_INFINITE, 0);
            clear_interrupt();
            drain_input();
            rx_handle_close(proc);
            return 130; // as for SIGINT in a Unix shell
        }
    }
    int64_t code = rx_wait_process(proc);
    if (code < 0) {
        const char *why = code == RX_TASK_RETCODE_SYSCALL_KILL ? "killed"
                        : code == RX_TASK_RETCODE_EXCEPTION_KILL ? "crashed (CPU exception)" : "exit code";
        printf(ANSI_RED "[%s: %lld]" ANSI_RESET "\n", why, (long long)code);
    }
    return (int)code;
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

static int run_program(struct word *w, int nassign, int argc, char **argv) {
    char **env = child_env(w, nassign);
    rx_handle_t proc = 0;
    rx_status_t r = RX_ERR_NOT_FOUND;
    char path[512];
    if (strchr(argv[0], '/')) {
        r = rx_spawn(argv[0], argv, env, RX_SPAWN_CLONE_STDIO, 0, 0, &proc);
    } else {
        const char *dirs = get_var("PATH");
        if (!dirs) dirs = "/bin";
        while (r == RX_ERR_NOT_FOUND && *dirs) {
            size_t len = strcspn(dirs, ":");
            snprintf(path, sizeof path, "%.*s/%s", (int)len, dirs, argv[0]);
            r = rx_spawn(path, argv, env, RX_SPAWN_CLONE_STDIO, 0, 0, &proc);
            dirs += len + (dirs[len] == ':');
        }
    }
    free(env);
    (void)argc;
    if (r == RX_ERR_NOT_FOUND) {
        fprintf(stderr, ANSI_RED "shell: command not found: " ANSI_RESET "%s\n", argv[0]);
        return 127;
    }
    if (r != RX_OK) {
        fprintf(stderr, "shell: cannot start %s (error %lld)\n", argv[0], (long long)r);
        return 126;
    }
    return wait_foreground(proc);
}

// ---- built-in commands ----

static void cmd_help(void) {
    printf("\n" ANSI_CYAN "Built-in commands:" ANSI_RESET "\n"
           "  help                 this list\n"
           "  clear                clear the screen\n"
           "  export NAME[=VALUE]  put a variable in the environment of programs\n"
           "  unset NAME           remove a variable\n"
           "  set                  list all variables\n"
           "  restart-kbd          kill the keyboard driver (init starts a new one)\n"
           "  exit [CODE]          leave the shell (init starts a new one)\n"
           "\n" ANSI_CYAN "Programs in /bin:" ANSI_RESET "\n"
           "  echo, cat, env, true, false, hello, counter, args\n"
           "  tests: libctest, objtest, ipctest, spawntest, drvtest, smptest, spin (Ctrl+C)\n"
           "\n" ANSI_CYAN "Shell:" ANSI_RESET "\n"
           "  'single' and \"double\" quotes, \\ escapes, $NAME, ${NAME}, $? (last exit code),\n"
           "  NAME=value, NAME=value command. Ctrl+C stops a program, Ctrl+U clears the line.\n\n");
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
    } else if (!strcmp(c, "exit")) {
        int code = argc > 1 ? atoi(argv[1]) : last_status;
        printf(ANSI_GREEN "Exiting shell." ANSI_RESET "\n");
        exit(code);
    } else return 0;
    return 1;
}

static void execute(char *line) {
    struct word w[MAX_WORDS];
    int n = parse(line, w, MAX_WORDS);
    if (n < 0) {
        last_status = 2;
        return;
    }
    int nassign = 0;
    while (nassign < n && w[nassign].assignment) nassign++;
    if (nassign == n) {
        // Only assignments: set shell variables
        for (int i = 0; i < n; i++) {
            char *eq = strchr(w[i].text, '=');
            *eq = 0;
            set_var(w[i].text, eq + 1);
        }
        if (n) last_status = 0;
    } else {
        char *argv[MAX_WORDS + 1];
        int argc = 0;
        for (int i = nassign; i < n; i++) argv[argc++] = w[i].text;
        argv[argc] = 0;
        int status;
        if (!builtin(argc, argv, &status)) status = run_program(w, nassign, argc, argv);
        last_status = status;
    }
    for (int i = 0; i < n; i++) free(w[i].text);
}

// ---- input ----

// Read one line from the console. Returns its length, 0 at end of input
// (Ctrl+D on an empty line), -1 if Ctrl+C was pressed.
static int read_line(char *buf, size_t cap) {
    rx_handle_t in = rx_fd_handle(0);
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
    init_ctl = rx_take_startup_handle(RX_PA_USER0);
    if (!rx_fd_handle(0)) {
        fprintf(stderr, "shell: no console\n");
        return 1;
    }
    show_welcome();
    for (;;) {
        printf(ANSI_MAGENTA "rustux" ANSI_RESET " " ANSI_CYAN ">" ANSI_RESET " ");
        fflush(stdout);
        int n = read_line(line, sizeof line);
        if (n < 0) continue; // Ctrl+C: new prompt
        if (n == 0) {
            if (!(rx_object_wait_one(rx_fd_handle(0), RX_PEER_CLOSED, 0, 0) == RX_OK)) {
                printf("\nUse \"exit\" to leave the shell.\n");
                continue;
            }
            return 0; // the console is gone
        }
        execute(line);
    }
}
