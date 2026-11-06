// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Rustux Shell (/bin/shell, also /bin/sh)
//!
//! A POSIX-style shell (checklist steps 7, 8, 10 and 11):
//!
//! - quoting: '...' (literal), "..." ($ and ` expanded; \" \\ \$ \` escaped), \x
//! - expansions: $NAME, ${NAME}, ${NAME:-w} ${NAME:=w} ${NAME:+w} ${NAME:?w},
//!   ${#NAME}, ${NAME#p} ${NAME##p} ${NAME%p} ${NAME%%p}; $? $# $@ $* $0-$9 $$ $!;
//!   $(command) and `command`; $((arithmetic)); ~; file name patterns (* ? [..]);
//!   unquoted results are split into words at spaces, tabs and newlines
//! - commands: pipelines `a | b`, `! a`, lists `a ; b`, `a && b`, `a || b`,
//!   `a &` (in the background), redirections `<`, `>`, `>>`, `n>`, `n>>`,
//!   `n>&m`, `<<WORD` (here-documents), `{ ...; }`, `( ... )`,
//!   if/elif/else/fi, while/until/do/done, for NAME [in WORDS]; do ...; done,
//!   case WORD in PATTERN) ... ;; esac, functions NAME() { ...; }
//! - built-ins: cd pwd export unset set shift local return break continue
//!   exit source . eval read test [ echo true false : type alias unalias
//!   wait umask exec history help clear loadkeys restart-kbd
//! - scripts: `shell FILE [ARGS]`, `shell -c COMMANDS`, or a file starting
//!   with #!/bin/sh run as a program (the C library starts the interpreter);
//!   set -e (stop on failure), set -x (print commands)
//! - interactive: line editing, history and Tab completion (lineedit.h); the
//!   console is in raw mode while a line is typed, cooked while programs
//!   run; unfinished commands (an open `if`, quote or `|`) continue on the
//!   next line after "> "; Ctrl+C ends what runs in the foreground
//!
//! Without fork, a pipeline whose parts are all programs runs them at the
//! same time, connected by pipes; one with built-ins, functions or compound
//! commands in it runs its parts one after another, through files in /tmp.

#include <ctype.h>
#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <rustux/console.h>
#include <rustux/process.h>
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

#define LINE_MAX_LEN 4096

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
static int interactive;
static const char *script_name = "shell";
static int opt_errexit, opt_xtrace;

// Control flow out of loops and functions
static int breaking, continuing, returning, exiting;
static int exit_code;
static int interrupted;   // Ctrl+C: abandon what is running
static int loop_depth, func_depth;
static int no_errexit;    // inside a condition (if, while, &&, ||, !)
static long last_bg;      // $!

// ---- small helpers ----

static void *xmalloc(size_t n) {
    void *p = malloc(n ? n : 1);
    if (!p) {
        fprintf(stderr, "shell: out of memory\n");
        exit(2);
    }
    return p;
}

static char *xstrdup(const char *s) { return strcpy(xmalloc(strlen(s) + 1), s); }

static char *xstrndup(const char *s, size_t n) {
    char *p = xmalloc(n + 1);
    memcpy(p, s, n);
    p[n] = 0;
    return p;
}

struct buf { char *p; size_t len, cap; };

static void put(struct buf *b, const char *s, size_t n) {
    if (b->len + n + 1 > b->cap) {
        b->cap = (b->len + n + 1) * 2;
        char *q = realloc(b->p, b->cap);
        if (!q) exit(2);
        b->p = q;
    }
    memcpy(b->p + b->len, s, n);
    b->len += n;
    b->p[b->len] = 0;
}
static void putc_(struct buf *b, char c) { put(b, &c, 1); }
static void puts_(struct buf *b, const char *s) { put(b, s, strlen(s)); }

static int is_name_char(int c, int first) { return c == '_' || isalpha(c) || (!first && isdigit(c)); }

static int is_name(const char *s, size_t n) {
    if (n == 0 || !is_name_char((unsigned char)s[0], 1)) return 0;
    for (size_t i = 1; i < n; i++)
        if (!is_name_char((unsigned char)s[i], 0)) return 0;
    return 1;
}

// Has Ctrl+C been pressed? (checked between commands)
static int check_interrupt(void) {
    if (interrupted) return 1;
    if (console && rx_object_wait_one(console, RX_CONSOLE_SIGNAL_INTERRUPT, 0, 0) == RX_OK) {
        rx_object_signal(console, RX_CONSOLE_SIGNAL_INTERRUPT, 0);
        interrupted = 1;
        return 1;
    }
    return 0;
}

static int stopping(void) { return breaking || continuing || returning || exiting || interrupted; }

// ---- variables ----
//
// Every variable lives here; exported ones are also in the environment
// (setenv), which programs get. `local` in a function saves the old value
// on a stack, put back when the function returns.

struct var { char *name, *value; int exported; };
static struct var *vars;
static int nvars, capvars;

struct saved_var { char *name, *value; int existed, exported, depth; };
static struct saved_var *saved;
static int nsaved, capsaved;

static struct var *find_var(const char *name) {
    for (int i = 0; i < nvars; i++)
        if (!strcmp(vars[i].name, name)) return &vars[i];
    return 0;
}

static const char *get_var(const char *name) {
    struct var *v = find_var(name);
    return v ? v->value : 0;
}

static void set_var(const char *name, const char *value) {
    struct var *v = find_var(name);
    if (!v) {
        if (nvars == capvars) {
            capvars = capvars ? capvars * 2 : 64;
            vars = realloc(vars, (size_t)capvars * sizeof *vars);
        }
        v = &vars[nvars++];
        v->name = xstrdup(name);
        v->value = 0;
        v->exported = 0;
    }
    char *old = v->value;
    v->value = xstrdup(value);
    free(old);
    if (v->exported) setenv(name, value, 1);
}

static void unset_var(const char *name) {
    for (int i = 0; i < nvars; i++) {
        if (!strcmp(vars[i].name, name)) {
            if (vars[i].exported) unsetenv(name);
            free(vars[i].name);
            free(vars[i].value);
            vars[i] = vars[--nvars];
            return;
        }
    }
    unsetenv(name);
}

static void export_var(const char *name) {
    struct var *v = find_var(name);
    if (!v) {
        set_var(name, "");
        v = find_var(name);
    }
    v->exported = 1;
    setenv(name, v->value, 1);
}

static void make_local(const char *name) {
    if (nsaved == capsaved) {
        capsaved = capsaved ? capsaved * 2 : 32;
        saved = realloc(saved, (size_t)capsaved * sizeof *saved);
    }
    struct var *v = find_var(name);
    struct saved_var *s = &saved[nsaved++];
    s->name = xstrdup(name);
    s->existed = v != 0;
    s->value = v ? xstrdup(v->value) : 0;
    s->exported = v ? v->exported : 0;
    s->depth = func_depth;
}

static void restore_locals(void) {
    while (nsaved > 0 && saved[nsaved - 1].depth == func_depth) {
        struct saved_var *s = &saved[--nsaved];
        if (s->existed) {
            set_var(s->name, s->value);
            find_var(s->name)->exported = s->exported;
        } else {
            unset_var(s->name);
        }
        free(s->name);
        free(s->value);
    }
}

// The environment at start: all exported
static void import_environment(void) {
    extern char **environ;
    for (char **e = environ; *e; e++) {
        char *eq = strchr(*e, '=');
        if (!eq) continue;
        char *name = xstrndup(*e, (size_t)(eq - *e));
        if (is_name(name, strlen(name))) {
            set_var(name, eq + 1);
            find_var(name)->exported = 1;
        }
        free(name);
    }
}

// Positional parameters ($1 ...)
static char **pos_argv;
static int pos_argc;

// ---- functions and aliases ----

struct node;
struct func { char *name; struct node *body; };
static struct func funcs[128];
static int nfuncs;

static struct node *find_func(const char *name) {
    for (int i = 0; i < nfuncs; i++)
        if (!strcmp(funcs[i].name, name)) return funcs[i].body;
    return 0;
}

struct alias { char *name, *value; };
static struct alias aliases[64];
static int naliases;

static const char *find_alias(const char *name) {
    for (int i = 0; i < naliases; i++)
        if (!strcmp(aliases[i].name, name)) return aliases[i].value;
    return 0;
}

// ---- syntax tree ----

enum {
    N_SIMPLE, N_PIPE, N_AND, N_OR, N_SEQ, N_BG, N_NOT, N_IF, N_WHILE, N_UNTIL, N_FOR, N_CASE,
    N_GROUP, N_SUBSHELL, N_FUNC,
};

enum { R_IN, R_OUT, R_APPEND, R_DUP_OUT, R_DUP_IN, R_HEREDOC, R_RW };

struct redir {
    int fd, op;
    char *word;         // target (raw), or the here-document's text
    int heredoc_quoted; // no expansion in the here-document
    struct redir *next;
};

struct case_item {
    char **pats;
    int npats;
    struct node *body;
    struct case_item *next;
};

struct node {
    int type, line;
    char **words;       // simple command (raw words), for list, case word
    int nwords;
    struct redir *redirs;
    struct node **kids; // pipe stages, list parts
    int nkids;
    struct node *a, *b, *c; // if: cond/then/else; while: cond/body; for/func: body
    char *name;         // for variable, function name
    int has_in;         // for ... in
    struct case_item *cases;
};

static struct node *new_node(int type, int line) {
    struct node *n = calloc(1, sizeof *n);
    if (!n) exit(2);
    n->type = type;
    n->line = line;
    return n;
}

static void add_kid(struct node *n, struct node *k) {
    n->kids = realloc(n->kids, (size_t)(n->nkids + 1) * sizeof *n->kids);
    n->kids[n->nkids++] = k;
}

static void add_word(char ***w, int *n, char *s) {
    *w = realloc(*w, (size_t)(*n + 2) * sizeof **w);
    (*w)[(*n)++] = s;
    (*w)[*n] = 0;
}

static void free_node(struct node *n) {
    if (!n) return;
    for (int i = 0; i < n->nwords; i++) free(n->words[i]);
    free(n->words);
    for (struct redir *r = n->redirs, *nx; r; r = nx) {
        nx = r->next;
        free(r->word);
        free(r);
    }
    for (int i = 0; i < n->nkids; i++) free_node(n->kids[i]);
    free(n->kids);
    free_node(n->a);
    free_node(n->b);
    free_node(n->c);
    free(n->name);
    for (struct case_item *c = n->cases, *nx; c; c = nx) {
        nx = c->next;
        for (int i = 0; i < c->npats; i++) free(c->pats[i]);
        free(c->pats);
        free_node(c->body);
        free(c);
    }
    free(n);
}

// ---- lexer ----

enum {
    T_EOF, T_WORD, T_NL, T_SEMI, T_AMP, T_PIPE, T_AND, T_OR, T_LPAREN, T_RPAREN, T_DSEMI, T_REDIR,
};

struct lexer {
    const char *src;
    size_t pos, len;
    int line;
    int tok;            // the current token
    char *text;         // T_WORD: raw text
    int rfd, rop;       // T_REDIR
    int tok_line;
    struct redir *pending[16]; // here-documents waiting for the end of the line
    int npending;
    int incomplete;     // ran out of input inside something unfinished
    int error;
};

static int lx_peekc(struct lexer *l, size_t k) { return l->pos + k < l->len ? (unsigned char)l->src[l->pos + k] : 0; }

static void syntax_error(struct lexer *l, const char *what) {
    if (l->error || l->incomplete) return;
    l->error = 1;
    if (interactive) fprintf(stderr, "shell: syntax error near %s\n", what);
    else fprintf(stderr, "shell: %s: line %d: syntax error near %s\n", script_name, l->tok_line, what);
}

// Read the body of each pending here-document (the lines after the one
// that asked for them)
static void read_heredocs(struct lexer *l) {
    for (int i = 0; i < l->npending; i++) {
        struct redir *r = l->pending[i];
        int strip = r->op == -1; // <<- strips leading tabs
        r->op = R_HEREDOC;
        // The delimiter without quotes (quoting it means no expansion)
        struct buf d = { 0 };
        puts_(&d, "");
        for (const char *s = r->word; *s; s++) {
            if (*s == '\'' || *s == '"') { r->heredoc_quoted = 1; continue; }
            if (*s == '\\' && s[1]) { r->heredoc_quoted = 1; s++; }
            putc_(&d, *s);
        }
        struct buf body = { 0 };
        puts_(&body, "");
        int found = 0;
        while (l->pos < l->len) {
            size_t e = l->pos;
            while (e < l->len && l->src[e] != '\n') e++;
            const char *s = l->src + l->pos;
            size_t n = e - l->pos;
            if (strip) while (n && *s == '\t') { s++; n--; }
            l->pos = e < l->len ? e + 1 : e;
            l->line++;
            if (n == d.len && !memcmp(s, d.p, n)) {
                found = 1;
                break;
            }
            put(&body, s, n);
            putc_(&body, '\n');
        }
        if (!found) l->incomplete = 1;
        free(d.p);
        free(r->word);
        r->word = body.p;
    }
    l->npending = 0;
}

// Copy a quoted or nested construct into b: '...', "...", `...`, $(...),
// ${...}. Returns 0 if the input ends first.
static int skip_quoted(struct lexer *l, struct buf *b);

static int skip_until(struct lexer *l, struct buf *b, char close) {
    int depth = 1;
    char open = close == ')' ? '(' : close == '}' ? '{' : 0;
    while (l->pos < l->len) {
        char c = l->src[l->pos];
        if (c == '\\' && l->pos + 1 < l->len) {
            put(b, l->src + l->pos, 2);
            l->pos += 2;
            continue;
        }
        if (c == '\'' || c == '"' || c == '`' || (c == '$' && (lx_peekc(l, 1) == '(' || lx_peekc(l, 1) == '{'))) {
            if (!skip_quoted(l, b)) return 0;
            continue;
        }
        if (c == '\n') l->line++;
        putc_(b, c);
        l->pos++;
        if (open && c == open) depth++;
        else if (c == close && --depth == 0) return 1;
    }
    return 0;
}

static int skip_quoted(struct lexer *l, struct buf *b) {
    char c = l->src[l->pos];
    if (c == '\'') {
        putc_(b, c);
        l->pos++;
        while (l->pos < l->len && l->src[l->pos] != '\'') {
            if (l->src[l->pos] == '\n') l->line++;
            putc_(b, l->src[l->pos++]);
        }
        if (l->pos >= l->len) return 0;
        putc_(b, l->src[l->pos++]);
        return 1;
    }
    if (c == '"' || c == '`') {
        putc_(b, c);
        l->pos++;
        while (l->pos < l->len && l->src[l->pos] != c) {
            char d = l->src[l->pos];
            if (d == '\\' && l->pos + 1 < l->len) {
                put(b, l->src + l->pos, 2);
                l->pos += 2;
                continue;
            }
            if (c == '"' && (d == '`' || (d == '$' && (lx_peekc(l, 1) == '(' || lx_peekc(l, 1) == '{')))) {
                if (!skip_quoted(l, b)) return 0;
                continue;
            }
            if (d == '\n') l->line++;
            putc_(b, d);
            l->pos++;
        }
        if (l->pos >= l->len) return 0;
        putc_(b, l->src[l->pos++]);
        return 1;
    }
    // $( or ${ (or $(( ))
    put(b, l->src + l->pos, 2);
    char open = l->src[l->pos + 1];
    l->pos += 2;
    return skip_until(l, b, open == '(' ? ')' : '}');
}

static int is_meta(int c) {
    return c == ' ' || c == '\t' || c == '\n' || c == ';' || c == '&' || c == '|' || c == '<' || c == '>' ||
           c == '(' || c == ')' || c == 0;
}

static void next_token(struct lexer *l) {
    free(l->text);
    l->text = 0;
    for (;;) {
        while (l->pos < l->len && (l->src[l->pos] == ' ' || l->src[l->pos] == '\t')) l->pos++;
        if (l->pos + 1 < l->len && l->src[l->pos] == '\\' && l->src[l->pos + 1] == '\n') {
            l->pos += 2; // line continuation
            l->line++;
            continue;
        }
        if (l->pos < l->len && l->src[l->pos] == '#') {
            while (l->pos < l->len && l->src[l->pos] != '\n') l->pos++;
        }
        break;
    }
    l->tok_line = l->line;
    if (l->pos >= l->len) {
        l->tok = T_EOF;
        return;
    }
    char c = l->src[l->pos], d = (char)lx_peekc(l, 1);
    switch (c) {
    case '\n':
        l->pos++;
        l->line++;
        l->tok = T_NL;
        if (l->npending) read_heredocs(l);
        return;
    case ';':
        l->pos += d == ';' ? 2 : 1;
        l->tok = d == ';' ? T_DSEMI : T_SEMI;
        return;
    case '&':
        l->pos += d == '&' ? 2 : 1;
        l->tok = d == '&' ? T_AND : T_AMP;
        return;
    case '|':
        l->pos += d == '|' ? 2 : 1;
        l->tok = d == '|' ? T_OR : T_PIPE;
        return;
    case '(':
        l->pos++;
        l->tok = T_LPAREN;
        return;
    case ')':
        l->pos++;
        l->tok = T_RPAREN;
        return;
    }
    // A redirection, with an optional fd number in front
    size_t p = l->pos;
    int fd = -1;
    if (isdigit((unsigned char)c)) {
        size_t q = p;
        while (q < l->len && isdigit((unsigned char)l->src[q])) q++;
        if (q < l->len && (l->src[q] == '<' || l->src[q] == '>')) {
            fd = atoi(l->src + p);
            p = q;
        }
    }
    if (p < l->len && (l->src[p] == '<' || l->src[p] == '>')) {
        char r = l->src[p];
        char r1 = p + 1 < l->len ? l->src[p + 1] : 0;
        char r2 = p + 2 < l->len ? l->src[p + 2] : 0;
        if (r == '>') {
            if (r1 == '>') { l->rop = R_APPEND; p += 2; }
            else if (r1 == '&') { l->rop = R_DUP_OUT; p += 2; }
            else if (r1 == '|') { l->rop = R_OUT; p += 2; }
            else { l->rop = R_OUT; p += 1; }
            if (fd < 0) fd = 1;
        } else {
            if (r1 == '<' && r2 == '-') { l->rop = -1; p += 3; }
            else if (r1 == '<') { l->rop = R_HEREDOC; p += 2; }
            else if (r1 == '&') { l->rop = R_DUP_IN; p += 2; }
            else if (r1 == '>') { l->rop = R_RW; p += 2; }
            else { l->rop = R_IN; p += 1; }
            if (fd < 0) fd = 0;
        }
        l->pos = p;
        l->rfd = fd;
        l->tok = T_REDIR;
        return;
    }
    // A word
    struct buf b = { 0 };
    while (l->pos < l->len && !is_meta((unsigned char)l->src[l->pos])) {
        char ch = l->src[l->pos];
        if (ch == '\\') {
            if (l->pos + 1 >= l->len) {
                putc_(&b, ch);
                l->pos++;
                break;
            }
            if (l->src[l->pos + 1] == '\n') { // continuation inside a word
                l->pos += 2;
                l->line++;
                continue;
            }
            put(&b, l->src + l->pos, 2);
            l->pos += 2;
            continue;
        }
        if (ch == '\'' || ch == '"' || ch == '`' || (ch == '$' && (lx_peekc(l, 1) == '(' || lx_peekc(l, 1) == '{'))) {
            if (!skip_quoted(l, &b)) {
                l->incomplete = 1;
                free(b.p);
                l->tok = T_EOF;
                return;
            }
            continue;
        }
        putc_(&b, ch);
        l->pos++;
    }
    l->text = b.p ? b.p : xstrdup("");
    l->tok = T_WORD;
}

static int is_word(struct lexer *l, const char *w) { return l->tok == T_WORD && !strcmp(l->text, w); }

static const char *tok_name(struct lexer *l) {
    static char buf[64];
    switch (l->tok) {
    case T_EOF: return "end of input";
    case T_NL: return "newline";
    case T_SEMI: return "';'";
    case T_AMP: return "'&'";
    case T_PIPE: return "'|'";
    case T_AND: return "'&&'";
    case T_OR: return "'||'";
    case T_LPAREN: return "'('";
    case T_RPAREN: return "')'";
    case T_DSEMI: return "';;'";
    case T_REDIR: return "a redirection";
    default:
        snprintf(buf, sizeof buf, "'%.40s'", l->text);
        return buf;
    }
}

// ---- parser ----

static struct node *parse_list(struct lexer *l, const char *const *enders);

static int is_reserved(const char *w) {
    static const char *const r[] = { "if", "then", "elif", "else", "fi", "while", "until", "do", "done", "for",
                                     "in", "case", "esac", "{", "}", "!", 0 };
    for (int i = 0; r[i]; i++)
        if (!strcmp(w, r[i])) return 1;
    return 0;
}

static void skip_newlines(struct lexer *l) {
    while (l->tok == T_NL) next_token(l);
}

// At the end of input something is unfinished; else a syntax error
static void unexpected(struct lexer *l) {
    if (l->tok == T_EOF) l->incomplete = 1;
    else syntax_error(l, tok_name(l));
}

static int expect_word(struct lexer *l, const char *w) {
    if (is_word(l, w)) {
        next_token(l);
        return 1;
    }
    if (l->tok == T_EOF) l->incomplete = 1;
    else {
        char what[96];
        snprintf(what, sizeof what, "%s (expected '%s')", tok_name(l), w);
        syntax_error(l, what);
    }
    return 0;
}

static int parse_redir(struct lexer *l, struct redir **list) {
    struct redir *r = calloc(1, sizeof *r);
    r->fd = l->rfd;
    r->op = l->rop;
    next_token(l);
    if (l->tok != T_WORD) {
        unexpected(l);
        free(r);
        return 0;
    }
    r->word = l->text;
    l->text = 0;
    if ((r->op == R_HEREDOC || r->op == -1) && l->npending < 16) l->pending[l->npending++] = r;
    struct redir **p = list;
    while (*p) p = &(*p)->next;
    *p = r;
    next_token(l);
    return 1;
}

static void parse_trailing_redirs(struct lexer *l, struct node *n) {
    while (l->tok == T_REDIR && !l->error && !l->incomplete) parse_redir(l, &n->redirs);
}

static const char *const END_THEN[] = { "then", 0 };
static const char *const END_ELSE[] = { "elif", "else", "fi", 0 };
static const char *const END_FI[] = { "fi", 0 };
static const char *const END_DO[] = { "do", 0 };
static const char *const END_DONE[] = { "done", 0 };
static const char *const END_BRACE[] = { "}", 0 };
static const char *const END_ESAC[] = { "esac", 0 };
static const char *const END_NONE[] = { 0 };

static struct node *parse_if(struct lexer *l) {
    struct node *n = new_node(N_IF, l->tok_line);
    next_token(l); // if / elif
    n->a = parse_list(l, END_THEN);
    if (!expect_word(l, "then")) return n;
    n->b = parse_list(l, END_ELSE);
    if (is_word(l, "elif")) {
        n->c = parse_if(l); // the inner if takes the "fi"
        return n;
    }
    if (is_word(l, "else")) {
        next_token(l);
        n->c = parse_list(l, END_FI);
    }
    expect_word(l, "fi");
    return n;
}

static struct node *parse_compound(struct lexer *l) {
    int line = l->tok_line;
    if (is_word(l, "if")) return parse_if(l);
    if (is_word(l, "while") || is_word(l, "until")) {
        struct node *n = new_node(is_word(l, "while") ? N_WHILE : N_UNTIL, line);
        next_token(l);
        n->a = parse_list(l, END_DO);
        if (!expect_word(l, "do")) return n;
        n->b = parse_list(l, END_DONE);
        expect_word(l, "done");
        return n;
    }
    if (is_word(l, "for")) {
        struct node *n = new_node(N_FOR, line);
        next_token(l);
        if (l->tok != T_WORD || !is_name(l->text, strlen(l->text))) {
            unexpected(l);
            return n;
        }
        n->name = l->text;
        l->text = 0;
        next_token(l);
        skip_newlines(l);
        if (is_word(l, "in")) {
            n->has_in = 1;
            next_token(l);
            while (l->tok == T_WORD) {
                add_word(&n->words, &n->nwords, l->text);
                l->text = 0;
                next_token(l);
            }
            if (l->tok == T_SEMI || l->tok == T_NL) next_token(l);
            else {
                unexpected(l);
                return n;
            }
        } else if (l->tok == T_SEMI) {
            next_token(l);
        }
        skip_newlines(l);
        if (!expect_word(l, "do")) return n;
        n->a = parse_list(l, END_DONE);
        expect_word(l, "done");
        return n;
    }
    if (is_word(l, "case")) {
        struct node *n = new_node(N_CASE, line);
        next_token(l);
        if (l->tok != T_WORD) {
            unexpected(l);
            return n;
        }
        add_word(&n->words, &n->nwords, l->text);
        l->text = 0;
        next_token(l);
        skip_newlines(l);
        if (!expect_word(l, "in")) return n;
        struct case_item **tail = &n->cases;
        for (;;) {
            skip_newlines(l);
            if (is_word(l, "esac")) {
                next_token(l);
                return n;
            }
            if (l->tok == T_EOF) {
                l->incomplete = 1;
                return n;
            }
            struct case_item *ci = calloc(1, sizeof *ci);
            *tail = ci;
            tail = &ci->next;
            if (l->tok == T_LPAREN) next_token(l);
            for (;;) {
                if (l->tok != T_WORD) {
                    unexpected(l);
                    return n;
                }
                add_word(&ci->pats, &ci->npats, l->text);
                l->text = 0;
                next_token(l);
                if (l->tok != T_PIPE) break;
                next_token(l);
            }
            if (l->tok != T_RPAREN) {
                unexpected(l);
                return n;
            }
            next_token(l);
            ci->body = parse_list(l, END_ESAC);
            if (l->error || l->incomplete) return n;
            if (l->tok == T_DSEMI) next_token(l);
            else if (!is_word(l, "esac")) {
                unexpected(l);
                return n;
            }
        }
    }
    if (is_word(l, "{")) {
        struct node *n = new_node(N_GROUP, line);
        next_token(l);
        n->a = parse_list(l, END_BRACE);
        expect_word(l, "}");
        return n;
    }
    if (l->tok == T_LPAREN) {
        struct node *n = new_node(N_SUBSHELL, line);
        next_token(l);
        n->a = parse_list(l, END_NONE);
        if (l->tok == T_RPAREN) next_token(l);
        else unexpected(l);
        return n;
    }
    return 0;
}

static struct node *parse_command(struct lexer *l) {
    if (l->error || l->incomplete) return 0;
    if (is_word(l, "!")) {
        struct node *n = new_node(N_NOT, l->tok_line);
        next_token(l);
        n->a = parse_command(l);
        return n;
    }
    struct node *c = parse_compound(l);
    if (c) {
        parse_trailing_redirs(l, c);
        return c;
    }
    struct node *n = new_node(N_SIMPLE, l->tok_line);
    while (!l->error && !l->incomplete) {
        if (l->tok == T_REDIR) {
            if (!parse_redir(l, &n->redirs)) break;
            continue;
        }
        if (l->tok != T_WORD) break;
        // A reserved word ends the command where a command would start
        if (n->nwords == 0 && is_reserved(l->text)) break;
        // NAME() { ... }: a function
        if (n->nwords == 0 && is_name(l->text, strlen(l->text)) && l->pos < l->len && l->src[l->pos] == '(') {
            size_t q = l->pos + 1;
            while (q < l->len && (l->src[q] == ' ' || l->src[q] == '\t')) q++;
            if (q < l->len && l->src[q] == ')') {
                struct node *f = new_node(N_FUNC, l->tok_line);
                f->name = l->text;
                l->text = 0;
                l->pos = q + 1;
                next_token(l);
                skip_newlines(l);
                f->a = parse_compound(l);
                if (!f->a) unexpected(l);
                else parse_trailing_redirs(l, f->a);
                free_node(n);
                return f;
            }
        }
        add_word(&n->words, &n->nwords, l->text);
        l->text = 0;
        next_token(l);
    }
    if (n->nwords == 0 && !n->redirs) {
        if (!l->error && !l->incomplete) unexpected(l);
        free_node(n);
        return 0;
    }
    return n;
}

static struct node *parse_pipeline(struct lexer *l) {
    struct node *first = parse_command(l);
    if (!first || l->tok != T_PIPE) return first;
    struct node *p = new_node(N_PIPE, first->line);
    add_kid(p, first);
    while (l->tok == T_PIPE && !l->error && !l->incomplete) {
        next_token(l);
        skip_newlines(l);
        struct node *c = parse_command(l);
        if (!c) break;
        add_kid(p, c);
    }
    return p;
}

static struct node *parse_and_or(struct lexer *l) {
    struct node *left = parse_pipeline(l);
    while (left && (l->tok == T_AND || l->tok == T_OR) && !l->error && !l->incomplete) {
        struct node *n = new_node(l->tok == T_AND ? N_AND : N_OR, left->line);
        next_token(l);
        skip_newlines(l);
        n->a = left;
        n->b = parse_pipeline(l);
        left = n;
        if (!n->b) break;
    }
    return left;
}

// A list up to one of `enders` (reserved words), ')', ';;' or the end
static struct node *parse_list(struct lexer *l, const char *const *enders) {
    struct node *seq = new_node(N_SEQ, l->tok_line);
    for (;;) {
        skip_newlines(l);
        if (l->error || l->incomplete || l->tok == T_EOF || l->tok == T_RPAREN || l->tok == T_DSEMI) break;
        int end = 0;
        for (int i = 0; enders[i]; i++) end |= is_word(l, enders[i]);
        if (end) break;
        struct node *c = parse_and_or(l);
        if (!c) break;
        if (l->tok == T_AMP) {
            struct node *bg = new_node(N_BG, c->line);
            bg->a = c;
            c = bg;
            next_token(l);
        } else if (l->tok == T_SEMI || l->tok == T_NL) {
            next_token(l);
        } else if (l->tok != T_EOF && l->tok != T_RPAREN && l->tok != T_DSEMI && l->tok != T_WORD) {
            syntax_error(l, tok_name(l));
        }
        add_kid(seq, c);
    }
    return seq;
}

// Parse a whole program. Returns 0 when it needs more input (*incomplete)
// or after a syntax error.
static struct node *parse_program(const char *src, int *incomplete) {
    struct lexer l = { 0 };
    l.src = src;
    l.len = strlen(src);
    l.line = 1;
    next_token(&l);
    struct node *n = parse_list(&l, END_NONE);
    if (!l.error && !l.incomplete && l.tok != T_EOF) syntax_error(&l, tok_name(&l));
    if (!l.error && !l.incomplete && l.npending) l.incomplete = 1;
    free(l.text);
    *incomplete = l.incomplete && !l.error;
    if (l.error || l.incomplete) {
        free_node(n);
        return 0;
    }
    return n;
}

// ---- expansion ----
//
// A word becomes fields: text with a "quoted" mark per byte, so splitting
// and file name patterns act only on what was not quoted.

struct field { struct buf s, q; int had_quotes; };
struct fields { struct field *f; int n, cap; };

static void fld_new(struct fields *fs) {
    if (fs->n == fs->cap) {
        fs->cap = fs->cap ? fs->cap * 2 : 8;
        fs->f = realloc(fs->f, (size_t)fs->cap * sizeof *fs->f);
    }
    memset(&fs->f[fs->n++], 0, sizeof *fs->f);
}

static void fld_put(struct fields *fs, const char *s, size_t n, int quoted) {
    if (!fs->n) fld_new(fs);
    struct field *f = &fs->f[fs->n - 1];
    put(&f->s, s, n);
    for (size_t i = 0; i < n; i++) putc_(&f->q, quoted ? 1 : 0);
    if (quoted) f->had_quotes = 1;
}

static void fld_free(struct fields *fs) {
    for (int i = 0; i < fs->n; i++) {
        free(fs->f[i].s.p);
        free(fs->f[i].q.p);
    }
    free(fs->f);
    memset(fs, 0, sizeof *fs);
}

static char *run_capture(const char *src);
static long arith(const char *expr, int *err);
static int match_q(const char *p, const char *q, const char *s);
static char *expand_string(const char *raw, int heredoc);
static int expand_failed;

static int match_pattern(const char *pat, const char *s) { return match_q(pat, 0, s); }

// Expanded text into the fields: quoted as is, unquoted split at IFS
static void fld_put_split(struct fields *fs, const char *s, int quoted) {
    if (quoted) {
        fld_put(fs, s, strlen(s), 1);
        return;
    }
    const char *ifs = get_var("IFS");
    if (!ifs) ifs = " \t\n";
    if (!fs->n) fld_new(fs);
    while (*s) {
        if (strchr(ifs, *s)) {
            while (*s && strchr(ifs, *s)) s++;
            struct field *f = &fs->f[fs->n - 1];
            if (f->s.len || f->had_quotes) fld_new(fs);
            continue;
        }
        size_t n = 0;
        while (s[n] && !strchr(ifs, s[n])) n++;
        fld_put(fs, s, n, 0);
        s += n;
    }
}

// The value of a parameter by name ($1, $?, $HOME...); 0 if unset
static const char *param_value(const char *name) {
    static char buf[32];
    static struct buf all;
    if (!name[0]) return 0;
    if (!name[1]) {
        switch (name[0]) {
        case '?': snprintf(buf, sizeof buf, "%d", last_status); return buf;
        case '#': snprintf(buf, sizeof buf, "%d", pos_argc); return buf;
        case '$': snprintf(buf, sizeof buf, "%d", (int)getpid()); return buf;
        case '!': snprintf(buf, sizeof buf, "%ld", last_bg); return buf;
        case '-':
            snprintf(buf, sizeof buf, "%s%s%s", opt_errexit ? "e" : "", opt_xtrace ? "x" : "", interactive ? "i" : "");
            return buf;
        case '@':
        case '*':
            all.len = 0;
            puts_(&all, "");
            for (int i = 0; i < pos_argc; i++) {
                if (i) putc_(&all, ' ');
                puts_(&all, pos_argv[i]);
            }
            return all.p;
        }
    }
    if (isdigit((unsigned char)name[0])) {
        int k = atoi(name);
        return k == 0 ? script_name : k <= pos_argc ? pos_argv[k - 1] : 0;
    }
    return get_var(name);
}

// Remove the shortest/longest prefix/suffix matching pat
static char *trim_pattern(const char *v, const char *pat, int suffix, int longest) {
    size_t n = strlen(v);
    if (!suffix) {
        char *tmp = xmalloc(n + 1);
        for (size_t k = 0; k <= n; k++) {
            size_t i = longest ? n - k : k;
            memcpy(tmp, v, i);
            tmp[i] = 0;
            if (match_pattern(pat, tmp)) {
                free(tmp);
                return xstrdup(v + i);
            }
        }
        free(tmp);
    } else {
        for (size_t k = 0; k <= n; k++) {
            size_t i = longest ? k : n - k; // where the suffix starts
            if (match_pattern(pat, v + i)) return xstrndup(v, i);
        }
    }
    return xstrdup(v);
}

// The body of ${...}
static char *expand_brace(const char *body) {
    if (body[0] == '#' && body[1]) { // ${#name}
        const char *v = param_value(body + 1);
        char buf[32];
        snprintf(buf, sizeof buf, "%zu", v ? strlen(v) : 0);
        return xstrdup(buf);
    }
    size_t n = 0;
    if (isdigit((unsigned char)body[0])) while (isdigit((unsigned char)body[n])) n++;
    else if (body[0] && strchr("?#$!@*-", body[0])) n = 1;
    else while (body[n] && is_name_char((unsigned char)body[n], n == 0)) n++;
    char *name = xstrndup(body, n);
    const char *v = param_value(name);
    const char *op = body + n;
    char *result;
    if (!*op) {
        result = xstrdup(v ? v : "");
    } else {
        int colon = *op == ':';
        if (colon) op++;
        char kind = *op;
        const char *word = op + 1;
        int unset = !v || (colon && !*v);
        if (kind == '+') {
            result = unset ? xstrdup("") : expand_string(word, 0);
        } else if (kind == '-' || kind == '=' || kind == '?') {
            if (!unset) {
                result = xstrdup(v);
            } else if (kind == '-') {
                result = expand_string(word, 0);
            } else if (kind == '=') {
                result = expand_string(word, 0);
                if (is_name(name, strlen(name))) set_var(name, result);
            } else {
                char *msg = expand_string(word, 0);
                fprintf(stderr, "shell: %s: %s\n", name, *msg ? msg : "parameter not set");
                free(msg);
                expand_failed = 1;
                result = xstrdup("");
            }
        } else if (kind == '#' || kind == '%') {
            int longest = op[1] == kind;
            char *pat = expand_string(op + 1 + longest, 0);
            result = trim_pattern(v ? v : "", pat, kind == '%', longest);
            free(pat);
        } else {
            fprintf(stderr, "shell: ${%s}: bad substitution\n", body);
            expand_failed = 1;
            result = xstrdup("");
        }
    }
    free(name);
    return result;
}

// The end of $( ... ) / ${ ... } / ` ... ` (s: after the opening); points
// at the closing character (or the end of the text)
static const char *find_close(const char *s, char close) {
    int depth = 1;
    char open = close == ')' ? '(' : close == '}' ? '{' : 0;
    while (*s) {
        if (*s == '\\' && s[1]) { s += 2; continue; }
        if (close != '`' && *s == '\'') {
            s++;
            while (*s && *s != '\'') s++;
            if (*s) s++;
            continue;
        }
        if (close != '`' && *s == '"') {
            s++;
            while (*s && *s != '"') {
                if (*s == '\\' && s[1]) s++;
                else if (*s == '$' && (s[1] == '(' || s[1] == '{')) {
                    s = find_close(s + 2, s[1] == '(' ? ')' : '}');
                    if (!*s) return s;
                }
                s++;
            }
            if (*s) s++;
            continue;
        }
        if (close != '`' && *s == '$' && (s[1] == '(' || s[1] == '{')) {
            s = find_close(s + 2, s[1] == '(' ? ')' : '}');
            if (*s) s++;
            continue;
        }
        if (open && *s == open) depth++;
        else if (*s == close && --depth == 0) return s;
        s++;
    }
    return s;
}

static void put_params_quoted(struct fields *fs) {
    // "$@": one field per parameter
    for (int i = 0; i < pos_argc; i++) {
        if (i) fld_new(fs);
        fld_put(fs, pos_argv[i], strlen(pos_argv[i]), 1);
    }
    if (pos_argc == 0 && fs->n && !fs->f[fs->n - 1].s.len) fs->f[fs->n - 1].had_quotes = 0;
}

// Expand the $... at *ps into the fields; advances *ps
static void expand_dollar(const char **ps, struct fields *fs, int quoted) {
    const char *s = *ps + 1; // after '$'
    if (*s == '(' && s[1] == '(') { // $(( arithmetic ))
        const char *e = find_close(s + 2, ')');
        char *expr = xstrndup(s + 2, (size_t)(e - s - 2));
        char *ex = expand_string(expr, 1);
        int err = 0;
        long v = arith(ex, &err);
        if (err) expand_failed = 1;
        char buf[32];
        snprintf(buf, sizeof buf, "%ld", v);
        fld_put_split(fs, buf, quoted);
        free(expr);
        free(ex);
        *ps = *e && e[1] ? e + 2 : e + (*e ? 1 : 0);
        return;
    }
    if (*s == '(') {
        const char *e = find_close(s + 1, ')');
        char *cmd = xstrndup(s + 1, (size_t)(e - s - 1));
        char *out = run_capture(cmd);
        fld_put_split(fs, out, quoted);
        free(cmd);
        free(out);
        *ps = *e ? e + 1 : e;
        return;
    }
    if (*s == '{') {
        const char *e = find_close(s + 1, '}');
        char *body = xstrndup(s + 1, (size_t)(e - s - 1));
        if (!strcmp(body, "@") && quoted) {
            put_params_quoted(fs);
        } else {
            char *v = expand_brace(body);
            fld_put_split(fs, v, quoted);
            free(v);
        }
        free(body);
        *ps = *e ? e + 1 : e;
        return;
    }
    if (*s == '@' && quoted) {
        put_params_quoted(fs);
        *ps = s + 1;
        return;
    }
    if (*s && (strchr("?#$!@*-", *s) || isdigit((unsigned char)*s))) {
        char name[2] = { *s, 0 };
        if (*s == '@' || *s == '*') {
            for (int i = 0; i < pos_argc; i++) {
                if (i) fld_put_split(fs, " ", 0);
                fld_put_split(fs, pos_argv[i], quoted);
            }
        } else {
            const char *v = param_value(name);
            if (v) fld_put_split(fs, v, quoted);
        }
        *ps = s + 1;
        return;
    }
    size_t n = 0;
    while (s[n] && is_name_char((unsigned char)s[n], n == 0)) n++;
    if (n == 0) { // a lone '$'
        fld_put(fs, "$", 1, quoted);
        *ps = s;
        return;
    }
    char *name = xstrndup(s, n);
    const char *v = get_var(name);
    free(name);
    if (v) fld_put_split(fs, v, quoted);
    *ps = s + n;
}

// Expand a word into fields (file name patterns come later). With
// `assignment`, unquoted expansions are not split.
static void expand_word_fields(const char *w, struct fields *fs, int assignment) {
    fld_new(fs);
    const char *s = w;
    if (*s == '~' && (s[1] == 0 || s[1] == '/')) {
        const char *home = get_var("HOME");
        if (!home) home = "~";
        fld_put(fs, home, strlen(home), 1);
        s++;
    }
    while (*s) {
        char c = *s;
        if (c == '\\') {
            if (s[1]) fld_put(fs, s + 1, 1, 1);
            s += s[1] ? 2 : 1;
        } else if (c == '\'') {
            const char *e = strchr(s + 1, '\'');
            if (!e) e = s + strlen(s);
            fld_put(fs, s + 1, (size_t)(e - s - 1), 1);
            fs->f[fs->n - 1].had_quotes = 1;
            s = *e ? e + 1 : e;
        } else if (c == '"') {
            s++;
            fs->f[fs->n - 1].had_quotes = 1;
            while (*s && *s != '"') {
                if (*s == '\\' && s[1] && strchr("$`\"\\\n", s[1])) {
                    if (s[1] != '\n') fld_put(fs, s + 1, 1, 1);
                    s += 2;
                } else if (*s == '$') {
                    expand_dollar(&s, fs, 1);
                } else if (*s == '`') {
                    const char *e = find_close(s + 1, '`');
                    char *cmd = xstrndup(s + 1, (size_t)(e - s - 1));
                    char *out = run_capture(cmd);
                    fld_put(fs, out, strlen(out), 1);
                    free(cmd);
                    free(out);
                    s = *e ? e + 1 : e;
                } else {
                    fld_put(fs, s, 1, 1);
                    s++;
                }
            }
            if (*s) s++;
        } else if (c == '$') {
            expand_dollar(&s, fs, assignment);
        } else if (c == '`') {
            const char *e = find_close(s + 1, '`');
            char *cmd = xstrndup(s + 1, (size_t)(e - s - 1));
            char *out = run_capture(cmd);
            fld_put_split(fs, out, assignment);
            free(cmd);
            free(out);
            s = *e ? e + 1 : e;
        } else {
            fld_put(fs, s, 1, 0);
            s++;
        }
    }
}

static char *join_fields(struct fields *fs) {
    struct buf b = { 0 };
    puts_(&b, "");
    for (int i = 0; i < fs->n; i++) {
        if (i) putc_(&b, ' ');
        if (fs->f[i].s.p) puts_(&b, fs->f[i].s.p);
    }
    return b.p;
}

// Expand a word to one string (no splitting, no patterns): assignments,
// redirection targets, case words; here-documents (heredoc: only $, `, \)
static char *expand_string(const char *raw, int heredoc) {
    if (!heredoc) {
        struct fields fs = { 0 };
        expand_word_fields(raw, &fs, 1);
        char *r = join_fields(&fs);
        fld_free(&fs);
        return r;
    }
    struct buf b = { 0 };
    puts_(&b, "");
    const char *s = raw;
    while (*s) {
        if (*s == '\\' && s[1] && strchr("$`\\\n", s[1])) {
            if (s[1] != '\n') putc_(&b, s[1]);
            s += 2;
        } else if (*s == '$' || *s == '`') {
            struct fields fs = { 0 };
            if (*s == '$') {
                expand_dollar(&s, &fs, 1);
            } else {
                const char *e = find_close(s + 1, '`');
                char *cmd = xstrndup(s + 1, (size_t)(e - s - 1));
                char *out = run_capture(cmd);
                fld_put(&fs, out, strlen(out), 1);
                free(cmd);
                free(out);
                s = *e ? e + 1 : e;
            }
            char *r = join_fields(&fs);
            puts_(&b, r);
            free(r);
            fld_free(&fs);
        } else {
            putc_(&b, *s++);
        }
    }
    return b.p;
}

// ---- file name patterns ----

// Does s match the pattern (* ? [...])? q marks quoted (literal)
// characters of the pattern; NULL: none.
static int match_q(const char *p, const char *q, const char *s) {
    while (*p) {
        int lit = q && *q;
        if (!lit && *p == '*') {
            while (*p == '*' && !(q && *q)) {
                p++;
                if (q) q++;
            }
            if (!*p) return 1;
            for (const char *t = s;; t++) {
                if (match_q(p, q, t)) return 1;
                if (!*t) return 0;
            }
        }
        if (!*s) return 0;
        if (!lit && *p == '?') {
            p++;
            if (q) q++;
            s++;
            continue;
        }
        if (!lit && *p == '[') {
            const char *e = p + 1;
            if (*e == '!' || *e == '^') e++;
            if (*e == ']') e++;
            while (*e && *e != ']') e++;
            if (*e == ']') {
                int neg = p[1] == '!' || p[1] == '^';
                const char *c = p + 1 + neg;
                int hit = 0;
                if (*c == ']') { hit = *s == ']'; c++; }
                while (c < e) {
                    if (c + 2 < e && c[1] == '-') {
                        hit |= (unsigned char)*s >= (unsigned char)c[0] && (unsigned char)*s <= (unsigned char)c[2];
                        c += 3;
                    } else {
                        hit |= *c == *s;
                        c++;
                    }
                }
                if (hit == neg) return 0;
                if (q) q += (e - p) + 1;
                p = e + 1;
                s++;
                continue;
            }
        }
        if (!lit && *p == '\\' && p[1]) {
            p++;
            if (q) q++;
        }
        if (*p != *s) return 0;
        p++;
        if (q) q++;
        s++;
    }
    return !*s;
}

static int has_glob(struct field *f) {
    for (size_t i = 0; i < f->s.len; i++)
        if (!f->q.p[i] && (f->s.p[i] == '*' || f->s.p[i] == '?' || f->s.p[i] == '[')) return 1;
    return 0;
}

static int str_cmp(const void *a, const void *b) { return strcmp(*(char *const *)a, *(char *const *)b); }

static char *path_join(const char *base, const char *name) {
    struct buf b = { 0 };
    puts_(&b, base);
    if (*base && base[strlen(base) - 1] != '/') putc_(&b, '/');
    puts_(&b, name);
    return b.p;
}

// Expand a pattern with directories (a/*/b*): matches appended to out
static void glob_expand(const char *pat, const char *qm, char ***out, int *n) {
    char **cur = 0;
    int ncur = 0;
    const char *s = pat, *q = qm;
    add_word(&cur, &ncur, xstrdup(*s == '/' ? "/" : ""));
    while (*s == '/') { s++; q++; }
    while (*s && ncur) {
        size_t k = strcspn(s, "/");
        char *comp = xstrndup(s, k), *cq = xmalloc(k + 1);
        memcpy(cq, q, k);
        cq[k] = 0;
        s += k;
        q += k;
        int more = *s == '/';
        while (*s == '/') { s++; q++; }
        int pattern = 0;
        for (size_t j = 0; j < k; j++)
            if (!cq[j] && (comp[j] == '*' || comp[j] == '?' || comp[j] == '[')) pattern = 1;
        char **next = 0;
        int nnext = 0;
        for (int c = 0; c < ncur; c++) {
            if (!pattern) {
                add_word(&next, &nnext, path_join(cur[c], comp));
                continue;
            }
            DIR *d = opendir(*cur[c] ? cur[c] : ".");
            if (!d) continue;
            struct dirent *de;
            char **found = 0;
            int nfound = 0;
            while ((de = readdir(d))) {
                if (de->d_name[0] == '.' && comp[0] != '.') continue;
                if (!strcmp(de->d_name, ".") || !strcmp(de->d_name, "..")) continue;
                if (!match_q(comp, cq, de->d_name)) continue;
                if (more && de->d_type != DT_DIR) continue;
                add_word(&found, &nfound, path_join(cur[c], de->d_name));
            }
            closedir(d);
            if (nfound) qsort(found, (size_t)nfound, sizeof *found, str_cmp);
            for (int i = 0; i < nfound; i++) add_word(&next, &nnext, found[i]);
            free(found);
        }
        for (int c = 0; c < ncur; c++) free(cur[c]);
        free(cur);
        free(comp);
        free(cq);
        cur = next;
        ncur = nnext;
    }
    // Only names that exist (literal components were not checked)
    for (int c = 0; c < ncur; c++) {
        struct stat st;
        if (stat(cur[c], &st) == 0) add_word(out, n, cur[c]);
        else free(cur[c]);
    }
    free(cur);
}

// Expand a command's words into a NULL-terminated argv (malloc'd)
static char **expand_words(char **words, int nwords, int *argc) {
    char **out = calloc(1, sizeof *out);
    int n = 0;
    for (int i = 0; i < nwords; i++) {
        struct fields fs = { 0 };
        expand_word_fields(words[i], &fs, 0);
        for (int k = 0; k < fs.n; k++) {
            struct field *f = &fs.f[k];
            if (!f->s.len && !f->had_quotes) continue; // empty and unquoted: dropped
            if (f->s.len && has_glob(f)) {
                int before = n;
                glob_expand(f->s.p, f->q.p, &out, &n);
                if (n > before) continue;
            }
            add_word(&out, &n, xstrdup(f->s.p ? f->s.p : ""));
        }
        fld_free(&fs);
    }
    *argc = n;
    return out;
}

static void free_argv(char **v) {
    if (!v) return;
    for (char **p = v; *p; p++) free(*p);
    free(v);
}

// ---- arithmetic ($((...)), long integers) ----

struct ar { const char *s; int err; };

static long ar_expr(struct ar *a);

static void ar_ws(struct ar *a) { while (*a->s == ' ' || *a->s == '\t' || *a->s == '\n') a->s++; }

static void set_num(const char *name, long v) {
    char buf[32];
    snprintf(buf, sizeof buf, "%ld", v);
    set_var(name, buf);
}

static long ar_primary(struct ar *a) {
    ar_ws(a);
    if (*a->s == '(') {
        a->s++;
        long v = ar_expr(a);
        ar_ws(a);
        if (*a->s == ')') a->s++;
        else a->err = 1;
        return v;
    }
    if (*a->s == '-' && a->s[1] != '-') { a->s++; return -ar_primary(a); }
    if (*a->s == '+' && a->s[1] != '+') { a->s++; return ar_primary(a); }
    if (*a->s == '!') { a->s++; return !ar_primary(a); }
    if (*a->s == '~') { a->s++; return ~ar_primary(a); }
    if (isdigit((unsigned char)*a->s)) {
        char *e;
        long v = strtol(a->s, &e, 0);
        a->s = e;
        return v;
    }
    if (is_name_char((unsigned char)*a->s, 1)) {
        const char *st = a->s;
        while (is_name_char((unsigned char)*a->s, 0)) a->s++;
        char *name = xstrndup(st, (size_t)(a->s - st));
        ar_ws(a);
        const char *cur = get_var(name);
        long old = cur ? strtol(cur, 0, 0) : 0, v = old;
        static const char *const ops[] = { "+=", "-=", "*=", "/=", "%=", 0 };
        int assign = -1;
        for (int i = 0; ops[i]; i++)
            if (!strncmp(a->s, ops[i], 2)) assign = i;
        if (assign >= 0 || (*a->s == '=' && a->s[1] != '=')) {
            a->s += assign >= 0 ? 2 : 1;
            long r = ar_expr(a);
            switch (assign) {
            case 0: r = old + r; break;
            case 1: r = old - r; break;
            case 2: r = old * r; break;
            case 3: if (r) r = old / r; else a->err = 2; break;
            case 4: if (r) r = old % r; else a->err = 2; break;
            }
            set_num(name, r);
            v = r;
        } else if (!strncmp(a->s, "++", 2) || !strncmp(a->s, "--", 2)) {
            set_num(name, a->s[0] == '+' ? old + 1 : old - 1);
            a->s += 2;
        }
        free(name);
        return v;
    }
    a->err = 1;
    return 0;
}

static long ar_binary(struct ar *a, int prec) {
    static const struct { const char *op; int prec; } ops[] = {
        { "||", 1 }, { "&&", 2 }, { "==", 6 }, { "!=", 6 }, { "<=", 7 }, { ">=", 7 }, { "<<", 8 }, { ">>", 8 },
        { "|", 3 }, { "^", 4 }, { "&", 5 }, { "<", 7 }, { ">", 7 }, { "+", 9 }, { "-", 9 }, { "*", 10 },
        { "/", 10 }, { "%", 10 }, { 0, 0 },
    };
    long l = ar_primary(a);
    for (;;) {
        ar_ws(a);
        int k = -1;
        for (int i = 0; ops[i].op; i++) {
            if (!strncmp(a->s, ops[i].op, strlen(ops[i].op))) { k = i; break; }
        }
        if (k < 0 || ops[k].prec < prec) return l;
        const char *op = ops[k].op;
        a->s += strlen(op);
        long r = ar_binary(a, ops[k].prec + 1);
        if (!strcmp(op, "||")) l = l || r;
        else if (!strcmp(op, "&&")) l = l && r;
        else if (!strcmp(op, "==")) l = l == r;
        else if (!strcmp(op, "!=")) l = l != r;
        else if (!strcmp(op, "<=")) l = l <= r;
        else if (!strcmp(op, ">=")) l = l >= r;
        else if (!strcmp(op, "<<")) l = l << r;
        else if (!strcmp(op, ">>")) l = l >> r;
        else if (!strcmp(op, "|")) l = l | r;
        else if (!strcmp(op, "^")) l = l ^ r;
        else if (!strcmp(op, "&")) l = l & r;
        else if (!strcmp(op, "<")) l = l < r;
        else if (!strcmp(op, ">")) l = l > r;
        else if (!strcmp(op, "+")) l = l + r;
        else if (!strcmp(op, "-")) l = l - r;
        else if (!strcmp(op, "*")) l = l * r;
        else if (r == 0) { a->err = 2; l = 0; }
        else if (!strcmp(op, "/")) l = l / r;
        else l = l % r;
    }
}

static long ar_expr(struct ar *a) {
    long c = ar_binary(a, 1);
    ar_ws(a);
    if (*a->s == '?') {
        a->s++;
        long x = ar_expr(a);
        ar_ws(a);
        if (*a->s != ':') {
            a->err = 1;
            return 0;
        }
        a->s++;
        long y = ar_expr(a);
        return c ? x : y;
    }
    return c;
}

static long arith(const char *expr, int *err) {
    struct ar a = { expr, 0 };
    long v = ar_expr(&a);
    ar_ws(&a);
    if (*a.s && !a.err) a.err = 1;
    if (a.err) {
        fprintf(stderr, "shell: %s: %s\n", expr, a.err == 2 ? "division by zero" : "bad arithmetic expression");
        *err = 1;
    }
    return v;
}

// ---- running programs ----

static void drain_input(void) {
    char buf[4 + RX_CONSOLE_MAX_DATA];
    uint32_t n;
    while (console && rx_channel_read(console, 0, buf, 0, sizeof buf, 0, &n, 0) == RX_OK) {}
}

static void report(int64_t code) {
    if (code < 0) {
        const char *why = code == RX_TASK_RETCODE_SYSCALL_KILL ? "killed"
                        : code == RX_TASK_RETCODE_EXCEPTION_KILL ? "crashed (CPU exception)" : "exit code";
        fprintf(stderr, ANSI_RED "[%s: %lld]" ANSI_RESET "\n", why, (long long)code);
    }
}

// A Rustux exit code as a shell status (0-255; killed 137, crashed 139)
static int status_of(int64_t code) {
    if (code == RX_TASK_RETCODE_SYSCALL_KILL) return 137;
    if (code == RX_TASK_RETCODE_EXCEPTION_KILL) return 139;
    if (code < 0) return 1;
    return (int)(code & 0xFF);
}

#define MAX_STAGES 32

// Wait for processes (all in `job`), ending them all on Ctrl+C. Returns
// the status of the one at `last`.
static int wait_foreground(rx_handle_t job, rx_handle_t *procs, int n, int last) {
    int64_t codes[MAX_STAGES];
    int left = 0;
    for (int i = 0; i < n; i++) {
        codes[i] = 127;
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
            if (job) rx_task_kill(job);
            for (int i = 0; i < n; i++) {
                if (procs[i]) {
                    rx_task_kill(procs[i]);
                    rx_wait_process(procs[i]);
                }
                procs[i] = 0;
            }
            rx_object_signal(console, RX_CONSOLE_SIGNAL_INTERRUPT, 0);
            drain_input();
            interrupted = 1;
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
    if (interactive) report(codes[last]);
    return status_of(codes[last]);
}

// Find a program: as given if it has a '/', else in $PATH. Returns 0, or
// 127 (not found) / 126 (not runnable) after printing why (unless quiet).
static int find_program(const char *name, char *path, size_t cap, int quiet) {
    struct stat st;
    if (strchr(name, '/')) {
        snprintf(path, cap, "%s", name);
        if (stat(path, &st) < 0) {
            if (!quiet) fprintf(stderr, "shell: %s: %s\n", name, strerror(errno));
            return 127;
        }
    } else {
        const char *dirs = get_var("PATH");
        if (!dirs) dirs = "/bin";
        int found = 0;
        while (*dirs && !found) {
            size_t len = strcspn(dirs, ":");
            snprintf(path, cap, "%.*s/%s", (int)(len ? len : 1), len ? dirs : ".", name);
            found = stat(path, &st) == 0 && !S_ISDIR(st.st_mode);
            dirs += len + (dirs[len] == ':');
        }
        if (!found) {
            if (!quiet) {
                if (interactive) fprintf(stderr, ANSI_RED "shell: command not found: " ANSI_RESET "%s\n", name);
                else fprintf(stderr, "shell: %s: command not found\n", name);
            }
            return 127;
        }
    }
    if (S_ISDIR(st.st_mode)) {
        if (!quiet) fprintf(stderr, "shell: %s: is a directory\n", name);
        return 126;
    }
    return 0;
}

// Environment for a program: exported variables plus the command's
// NAME=value words
static char **child_env(char **assigns, int nassign) {
    extern char **environ;
    int n = 0;
    while (environ[n]) n++;
    char **env = xmalloc((size_t)(n + nassign + 1) * sizeof *env);
    int k = 0;
    for (int i = 0; i < n; i++) {
        size_t len = strcspn(environ[i], "=");
        int overridden = 0;
        for (int j = 0; j < nassign; j++)
            if (!strncmp(assigns[j], environ[i], len) && assigns[j][len] == '=') overridden = 1;
        if (!overridden) env[k++] = environ[i];
    }
    for (int j = 0; j < nassign; j++) env[k++] = assigns[j];
    env[k] = 0;
    return env;
}

// Start a program with our fds 0-2. A text file without #! runs as a
// shell script (the C library handles #!).
static rx_status_t start_program(rx_handle_t job, const char *path, char **argv, char **env, rx_handle_t *proc) {
    rx_spawn_handle_t extra[2];
    uint32_t nextra = 0;
    if (power && rx_handle_duplicate(power, RX_RIGHT_SAME_RIGHTS, &extra[nextra].handle) == RX_OK)
        extra[nextra++].info = RX_PA_POWER_RESOURCE;
    if (rx_handle_duplicate(rx_job_default, RX_RIGHT_SAME_RIGHTS, &extra[nextra].handle) == RX_OK)
        extra[nextra++].info = RX_PA_JOB_SYSTEM;
    char head[4] = { 0 };
    int fd = open(path, O_RDONLY);
    if (fd >= 0) {
        ssize_t k = read(fd, head, sizeof head);
        close(fd);
        if (k >= 1 && (k < 4 || memcmp(head, "\177ELF", 4)) && memcmp(head, "#!", 2)) {
            int n = 0;
            while (argv[n]) n++;
            char **v = xmalloc((size_t)(n + 2) * sizeof *v);
            v[0] = "/bin/shell";
            v[1] = (char *)path;
            for (int i = 1; i <= n; i++) v[i + 1] = argv[i];
            rx_status_t r = rx_spawn_etc(job, "/bin/shell", v, env, RX_SPAWN_CLONE_ALL, extra, nextra, proc);
            free(v);
            return r;
        }
    }
    return rx_spawn_etc(job, path, argv, env, RX_SPAWN_CLONE_ALL, extra, nextra, proc);
}

// ---- redirections ----

struct saved_fds { int fd[10]; int used; };

static void save_fd(struct saved_fds *s, int fd) {
    if (fd < 0 || fd > 9 || (s->used & (1 << fd))) return;
    s->fd[fd] = dup(fd);
    s->used |= 1 << fd;
}

static void restore_fds(struct saved_fds *s) {
    fflush(stdout);
    fflush(stderr);
    for (int fd = 0; fd < 10; fd++) {
        if (!(s->used & (1 << fd))) continue;
        if (s->fd[fd] >= 0) {
            dup2(s->fd[fd], fd);
            close(s->fd[fd]);
        } else {
            close(fd);
        }
    }
    s->used = 0;
}

static int tmp_counter;

static char *tmp_name(void) {
    char buf[64];
    snprintf(buf, sizeof buf, "/tmp/.sh-%d-%d", (int)getpid(), tmp_counter++);
    return xstrdup(buf);
}

// Apply redirections to our fds, saving the old ones in *s. Returns 0, or
// 1 after printing why one failed.
static int apply_redirs(struct redir *r, struct saved_fds *s) {
    fflush(stdout);
    fflush(stderr);
    for (; r; r = r->next) {
        if (r->fd > 9) {
            fprintf(stderr, "shell: fd %d: too large\n", r->fd);
            return 1;
        }
        save_fd(s, r->fd);
        if (r->op == R_HEREDOC) {
            char *text = r->heredoc_quoted ? xstrdup(r->word) : expand_string(r->word, 1);
            char *name = tmp_name();
            int fd = open(name, O_RDWR | O_CREAT | O_TRUNC, 0600);
            if (fd >= 0) {
                size_t n = strlen(text);
                if (write(fd, text, n) != (ssize_t)n) {}
                close(fd);
                fd = open(name, O_RDONLY);
            }
            unlink(name); // stays readable while open
            free(name);
            free(text);
            if (fd < 0) {
                fprintf(stderr, "shell: here-document: %s\n", strerror(errno));
                return 1;
            }
            dup2(fd, r->fd);
            close(fd);
            continue;
        }
        char *target = expand_string(r->word, 0);
        if (r->op == R_DUP_OUT || r->op == R_DUP_IN) {
            int bad = 0;
            if (!strcmp(target, "-")) close(r->fd);
            else if (isdigit((unsigned char)target[0])) bad = dup2(atoi(target), r->fd) < 0;
            else bad = 1;
            if (bad) {
                fprintf(stderr, "shell: %s: bad file descriptor\n", target);
                free(target);
                return 1;
            }
            free(target);
            continue;
        }
        int flags = r->op == R_IN ? O_RDONLY
                  : r->op == R_RW ? O_RDWR | O_CREAT
                  : r->op == R_APPEND ? O_WRONLY | O_CREAT | O_APPEND
                  : O_WRONLY | O_CREAT | O_TRUNC;
        int fd = open(target, flags, 0666);
        if (fd < 0) {
            fprintf(stderr, "shell: %s: %s\n", target, strerror(errno));
            free(target);
            return 1;
        }
        free(target);
        if (fd != r->fd) {
            dup2(fd, r->fd);
            close(fd);
        }
    }
    return 0;
}

// ---- execution ----

static int exec_node(struct node *n);
static int last_status_after_subst(void);
static int run_builtin(int argc, char **argv, int *status);
static int is_builtin(const char *name);
static int run_string(const char *src, const char *name);

// Run a function: its own positional parameters and local variables
static int call_function(struct node *body, int argc, char **argv) {
    char **old_argv = pos_argv;
    int old_argc = pos_argc;
    pos_argv = argv + 1;
    pos_argc = argc - 1;
    func_depth++;
    int st = exec_node(body);
    restore_locals();
    func_depth--;
    pos_argv = old_argv;
    pos_argc = old_argc;
    if (returning) {
        returning = 0;
        st = last_status;
    }
    return st;
}

static void xtrace(int argc, char **argv) {
    if (!opt_xtrace) return;
    fprintf(stderr, "+");
    for (int i = 0; i < argc; i++) fprintf(stderr, " %s", argv[i]);
    fprintf(stderr, "\n");
}

// How many leading words are NAME=value
static int count_assignments(char **words, int n) {
    int k = 0;
    while (k < n) {
        const char *eq = strchr(words[k], '=');
        if (!eq || !is_name(words[k], (size_t)(eq - words[k]))) break;
        k++;
    }
    return k;
}

// NAME=value words expanded (malloc'd strings)
static char **expand_assignments(char **words, int n) {
    char **out = xmalloc((size_t)(n + 1) * sizeof *out);
    for (int i = 0; i < n; i++) {
        const char *eq = strchr(words[i], '=');
        char *v = expand_string(eq + 1, 0);
        size_t nl = (size_t)(eq - words[i]);
        out[i] = xmalloc(nl + strlen(v) + 2);
        memcpy(out[i], words[i], nl + 1);
        strcpy(out[i] + nl + 1, v);
        free(v);
    }
    out[n] = 0;
    return out;
}

// The first word through aliases (one level)
static char **apply_alias(char **words, int nwords, int *nout) {
    const char *a = nwords > 0 ? find_alias(words[0]) : 0;
    if (!a) return 0;
    char **out = 0;
    int n = 0;
    struct lexer l = { 0 };
    l.src = a;
    l.len = strlen(a);
    for (next_token(&l); l.tok == T_WORD; next_token(&l)) {
        add_word(&out, &n, l.text);
        l.text = 0;
    }
    free(l.text);
    for (int i = 1; i < nwords; i++) add_word(&out, &n, xstrdup(words[i]));
    *nout = n;
    return out;
}

static int exec_simple(struct node *n, int background) {
    expand_failed = 0;
    int nassign = count_assignments(n->words, n->nwords);
    char **words = n->words + nassign;
    int nwords = n->nwords - nassign;
    int nalias = 0;
    char **alias_words = apply_alias(words, nwords, &nalias);
    if (alias_words) {
        words = alias_words;
        nwords = nalias;
    }
    int argc = 0;
    char **argv = expand_words(words, nwords, &argc);
    free_argv(alias_words);
    char **assigns = expand_assignments(n->words, nassign);
    int status = 0;
    if (expand_failed) {
        status = 1;
        goto out;
    }
    if (argc == 0) {
        // Only assignments (and redirections)
        for (int i = 0; i < nassign; i++) {
            char *eq = strchr(assigns[i], '=');
            *eq = 0;
            set_var(assigns[i], eq + 1);
            *eq = '=';
        }
        struct saved_fds s = { { 0 }, 0 };
        status = apply_redirs(n->redirs, &s);
        restore_fds(&s);
        if (status == 0 && nassign) status = last_status_after_subst();
        goto out;
    }
    xtrace(argc, argv);
    struct node *fn = find_func(argv[0]);
    if (fn || is_builtin(argv[0])) {
        struct saved_fds s = { { 0 }, 0 };
        if (apply_redirs(n->redirs, &s)) {
            restore_fds(&s);
            status = 1;
            goto out;
        }
        // NAME=value before a built-in or function: for its duration
        char **old = xmalloc((size_t)(nassign + 1) * sizeof *old);
        for (int i = 0; i < nassign; i++) {
            char *eq = strchr(assigns[i], '=');
            *eq = 0;
            const char *v = get_var(assigns[i]);
            old[i] = v ? xstrdup(v) : 0;
            set_var(assigns[i], eq + 1);
            *eq = '=';
        }
        if (fn) status = call_function(fn, argc, argv);
        else run_builtin(argc, argv, &status);
        fflush(stdout);
        fflush(stderr);
        for (int i = 0; i < nassign; i++) {
            char *eq = strchr(assigns[i], '=');
            *eq = 0;
            if (old[i]) set_var(assigns[i], old[i]);
            else unset_var(assigns[i]);
            *eq = '=';
            free(old[i]);
        }
        free(old);
        restore_fds(&s);
        goto out;
    }
    char path[PATH_MAX];
    status = find_program(argv[0], path, sizeof path, 0);
    if (status) goto out;
    struct saved_fds s = { { 0 }, 0 };
    if (apply_redirs(n->redirs, &s)) {
        restore_fds(&s);
        status = 1;
        goto out;
    }
    char **env = child_env(assigns, nassign);
    rx_handle_t job = 0, proc = 0;
    if (rx_job_create(rx_job_default, 0, &job) != RX_OK) job = 0;
    rx_status_t r = start_program(job ? job : rx_job_default, path, argv, env, &proc);
    free(env);
    restore_fds(&s);
    if (r != RX_OK) {
        fprintf(stderr, "shell: cannot start %s: %s\n", argv[0], strerror(errno));
        status = 126;
    } else if (background) {
        rx_info_handle_basic_t b;
        last_bg = rx_object_get_info(proc, RX_INFO_HANDLE_BASIC, &b, sizeof b, 0, 0) == RX_OK ? (long)b.koid : 0;
        if (interactive) fprintf(stderr, "[%ld] %s\n", last_bg, argv[0]);
        rx_handle_close(proc);
        status = 0;
    } else {
        status = wait_foreground(job, &proc, 1, 0);
    }
    if (job) rx_handle_close(job);
out:
    free_argv(assigns);
    free_argv(argv);
    return status;
}

// A pipeline of programs only: all at once, connected by pipes
static int pipe_concurrent(struct node *p, char ***argvs, int *argcs) {
    rx_handle_t job = 0, procs[MAX_STAGES] = { 0 };
    int status = 0, prev_read = -1;
    struct saved_fds outer = { { 0 }, 0 };
    fflush(stdout);
    save_fd(&outer, 0);
    save_fd(&outer, 1);
    save_fd(&outer, 2);
    if (rx_job_create(rx_job_default, 0, &job) != RX_OK) job = 0;
    for (int i = 0; i < p->nkids; i++) {
        struct node *st = p->kids[i];
        for (int k = 0; k < 3; k++) dup2(outer.fd[k], k);
        if (prev_read >= 0) {
            dup2(prev_read, 0);
            close(prev_read);
            prev_read = -1;
        }
        int pp[2] = { -1, -1 };
        if (i + 1 < p->nkids && pipe(pp) == 0) {
            dup2(pp[1], 1);
            close(pp[1]);
            prev_read = pp[0];
        }
        struct saved_fds s = { { 0 }, 0 };
        int st_status = apply_redirs(st->redirs, &s);
        if (st_status == 0) {
            char path[PATH_MAX];
            xtrace(argcs[i], argvs[i]);
            st_status = find_program(argvs[i][0], path, sizeof path, 0);
            if (st_status == 0) {
                int nassign = count_assignments(st->words, st->nwords);
                char **assigns = expand_assignments(st->words, nassign);
                char **env = child_env(assigns, nassign);
                if (start_program(job ? job : rx_job_default, path, argvs[i], env, &procs[i]) != RX_OK) {
                    fprintf(stderr, "shell: cannot start %s: %s\n", argvs[i][0], strerror(errno));
                    st_status = 126;
                    procs[i] = 0;
                }
                free(env);
                free_argv(assigns);
            }
        }
        // Drop this stage's saved copies (fds 0-2 are reset for the next)
        for (int k = 0; k < 10; k++) {
            if (!(s.used & (1 << k))) continue;
            if (k > 2 && s.fd[k] >= 0) dup2(s.fd[k], k);
            if (s.fd[k] >= 0) close(s.fd[k]);
        }
        if (i == p->nkids - 1) status = st_status;
    }
    if (prev_read >= 0) close(prev_read);
    restore_fds(&outer);
    int any = 0;
    for (int i = 0; i < p->nkids; i++) any |= procs[i] != 0;
    if (any) {
        int last_ran = procs[p->nkids - 1] != 0;
        int code = wait_foreground(job, procs, p->nkids, p->nkids - 1);
        if (last_ran || code == 130) status = code;
    }
    if (job) rx_handle_close(job);
    return status;
}

// A pipeline with built-ins, functions or compound commands in it: one
// part after another, each part's output in a file read by the next
static int pipe_sequential(struct node *p) {
    int status = 0;
    char *prev = 0;
    for (int i = 0; i < p->nkids && !stopping(); i++) {
        struct saved_fds s = { { 0 }, 0 };
        fflush(stdout);
        save_fd(&s, 0);
        save_fd(&s, 1);
        if (prev) {
            int fd = open(prev, O_RDONLY);
            if (fd >= 0) {
                dup2(fd, 0);
                close(fd);
            }
            unlink(prev);
            free(prev);
            prev = 0;
        }
        if (i + 1 < p->nkids) {
            prev = tmp_name();
            int fd = open(prev, O_RDWR | O_CREAT | O_TRUNC, 0600);
            if (fd >= 0) {
                dup2(fd, 1);
                close(fd);
            }
        }
        status = exec_node(p->kids[i]);
        restore_fds(&s);
    }
    if (prev) {
        unlink(prev);
        free(prev);
    }
    return status;
}

static int exec_pipe(struct node *p) {
    // Programs only? Expand each stage's words to find out.
    char **argvs[MAX_STAGES];
    int argcs[MAX_STAGES];
    int all = p->nkids <= MAX_STAGES;
    int k = 0;
    for (; all && k < p->nkids; k++) {
        struct node *st = p->kids[k];
        if (st->type != N_SIMPLE) { all = 0; break; }
        int nassign = count_assignments(st->words, st->nwords);
        if (nassign == st->nwords || find_alias(st->words[nassign])) { all = 0; break; }
        argvs[k] = expand_words(st->words + nassign, st->nwords - nassign, &argcs[k]);
        if (argcs[k] == 0 || is_builtin(argvs[k][0]) || find_func(argvs[k][0])) {
            free_argv(argvs[k]);
            all = 0;
            break;
        }
    }
    int status = all ? pipe_concurrent(p, argvs, argcs) : pipe_sequential(p);
    for (int i = 0; i < k && i < MAX_STAGES && all; i++) free_argv(argvs[i]);
    if (!all)
        for (int i = 0; i < k; i++) free_argv(argvs[i]);
    return status;
}

// ( ... ): changes to variables, functions' arguments and the directory
// stay inside
static int exec_subshell(struct node *n) {
    char cwd[PATH_MAX];
    if (!getcwd(cwd, sizeof cwd)) strcpy(cwd, "/");
    int ncopy = nvars;
    struct var *copy = xmalloc((size_t)(ncopy ? ncopy : 1) * sizeof *copy);
    for (int i = 0; i < nvars; i++) {
        copy[i].name = xstrdup(vars[i].name);
        copy[i].value = xstrdup(vars[i].value);
        copy[i].exported = vars[i].exported;
    }
    char **old_argv = pos_argv;
    int old_argc = pos_argc, e = opt_errexit, x = opt_xtrace;
    int st = exec_node(n->a);
    if (exiting) {
        exiting = 0;
        st = exit_code;
    }
    breaking = continuing = returning = 0;
    for (int i = 0; i < nvars; i++) {
        if (vars[i].exported) unsetenv(vars[i].name);
        free(vars[i].name);
        free(vars[i].value);
    }
    free(vars);
    vars = copy;
    nvars = capvars = ncopy ? ncopy : 1;
    nvars = ncopy;
    for (int i = 0; i < nvars; i++)
        if (vars[i].exported) setenv(vars[i].name, vars[i].value, 1);
    pos_argv = old_argv;
    pos_argc = old_argc;
    opt_errexit = e;
    opt_xtrace = x;
    if (chdir(cwd) < 0) {}
    return st;
}

static int exec_node(struct node *n) {
    if (!n || stopping()) return last_status;
    if (check_interrupt()) return 130;
    int st = 0;
    switch (n->type) {
    case N_SIMPLE:
        st = exec_simple(n, 0);
        break;
    case N_PIPE:
        st = exec_pipe(n);
        break;
    case N_NOT:
        no_errexit++;
        st = !exec_node(n->a);
        no_errexit--;
        break;
    case N_AND:
    case N_OR:
        no_errexit++;
        st = exec_node(n->a);
        no_errexit--;
        last_status = st;
        if (!stopping() && ((n->type == N_AND) == (st == 0))) st = exec_node(n->b);
        break;
    case N_SEQ:
        for (int i = 0; i < n->nkids && !stopping(); i++) {
            st = exec_node(n->kids[i]);
            last_status = st;
        }
        return st;
    case N_BG:
        // Programs go to the background; compound commands run here
        st = n->a->type == N_SIMPLE ? exec_simple(n->a, 1) : exec_node(n->a);
        break;
    case N_IF:
        no_errexit++;
        st = exec_node(n->a);
        no_errexit--;
        if (stopping()) break;
        if (st == 0) st = exec_node(n->b);
        else st = n->c ? exec_node(n->c) : 0;
        break;
    case N_WHILE:
    case N_UNTIL:
        st = 0;
        loop_depth++;
        for (;;) {
            no_errexit++;
            int c = exec_node(n->a);
            no_errexit--;
            if (stopping()) break;
            if ((c == 0) != (n->type == N_WHILE)) break;
            st = exec_node(n->b);
            if (continuing) {
                if (--continuing) break;
                continue;
            }
            if (breaking) {
                breaking--;
                break;
            }
            if (stopping() || check_interrupt()) break;
        }
        loop_depth--;
        break;
    case N_FOR: {
        int argc = 0;
        char **items;
        if (n->has_in) {
            items = expand_words(n->words, n->nwords, &argc);
        } else {
            items = calloc((size_t)pos_argc + 1, sizeof *items);
            for (int i = 0; i < pos_argc; i++) items[i] = xstrdup(pos_argv[i]);
            argc = pos_argc;
        }
        st = 0;
        loop_depth++;
        for (int i = 0; i < argc; i++) {
            set_var(n->name, items[i]);
            st = exec_node(n->a);
            if (continuing) {
                if (--continuing) break;
                continue;
            }
            if (breaking) {
                breaking--;
                break;
            }
            if (stopping() || check_interrupt()) break;
        }
        loop_depth--;
        free_argv(items);
        break;
    }
    case N_CASE: {
        char *w = expand_string(n->words[0], 0);
        st = 0;
        for (struct case_item *c = n->cases; c; c = c->next) {
            int hit = 0;
            for (int i = 0; i < c->npats && !hit; i++) {
                // The pattern expanded; its quoted parts match literally
                struct fields fs = { 0 };
                expand_word_fields(c->pats[i], &fs, 1);
                struct buf pb = { 0 }, qb = { 0 };
                puts_(&pb, "");
                put(&qb, "", 0);
                for (int k = 0; k < fs.n; k++) {
                    if (!fs.f[k].s.len) continue;
                    put(&pb, fs.f[k].s.p, fs.f[k].s.len);
                    put(&qb, fs.f[k].q.p, fs.f[k].q.len);
                }
                hit = match_q(pb.p, qb.p, w);
                free(pb.p);
                free(qb.p);
                fld_free(&fs);
            }
            if (hit) {
                st = c->body ? exec_node(c->body) : 0;
                break;
            }
        }
        free(w);
        break;
    }
    case N_GROUP:
    case N_SUBSHELL: {
        struct saved_fds s = { { 0 }, 0 };
        if (apply_redirs(n->redirs, &s)) {
            restore_fds(&s);
            st = 1;
            break;
        }
        st = n->type == N_GROUP ? exec_node(n->a) : exec_subshell(n);
        restore_fds(&s);
        break;
    }
    case N_FUNC: {
        int i;
        for (i = 0; i < nfuncs; i++)
            if (!strcmp(funcs[i].name, n->name)) break;
        if (i == nfuncs) {
            if (nfuncs == 128) {
                fprintf(stderr, "shell: too many functions\n");
                st = 1;
                break;
            }
            funcs[nfuncs].name = xstrdup(n->name);
            funcs[nfuncs++].body = 0;
        }
        // The body is kept: taken out of the tree, which is freed
        funcs[i].body = n->a;
        n->a = 0;
        st = 0;
        break;
    }
    }
    last_status = st;
    if (opt_errexit && st != 0 && !no_errexit && !stopping() &&
        (n->type == N_SIMPLE || n->type == N_PIPE || n->type == N_GROUP || n->type == N_SUBSHELL)) {
        exiting = 1;
        exit_code = st;
    }
    return st;
}

// Run shell source text; returns the last status. `name` names a script
// file for messages.
static int run_string(const char *src, const char *name) {
    int incomplete = 0;
    const char *old = script_name;
    if (name) script_name = name;
    struct node *n = parse_program(src, &incomplete);
    if (!n) {
        if (incomplete) fprintf(stderr, "shell: %s: unexpected end of input\n", name ? name : "command");
        script_name = old;
        last_status = 2;
        return 2;
    }
    int st = exec_node(n);
    free_node(n);
    script_name = old;
    return st;
}

// $(...): run with standard output into a file; what it wrote, without
// the trailing newlines
static char *run_capture(const char *src) {
    char *name = tmp_name();
    struct saved_fds s = { { 0 }, 0 };
    fflush(stdout);
    save_fd(&s, 1);
    int fd = open(name, O_RDWR | O_CREAT | O_TRUNC, 0600);
    if (fd >= 0) {
        dup2(fd, 1);
        close(fd);
    }
    int ex = exiting;
    run_string(src, 0);
    exiting = ex; // `exit` inside $( ) ends only it
    returning = breaking = continuing = 0;
    restore_fds(&s);
    struct buf b = { 0 };
    puts_(&b, "");
    fd = open(name, O_RDONLY);
    if (fd >= 0) {
        char chunk[4096];
        ssize_t k;
        while ((k = read(fd, chunk, sizeof chunk)) > 0) put(&b, chunk, (size_t)k);
        close(fd);
    }
    unlink(name);
    free(name);
    while (b.len && b.p[b.len - 1] == '\n') b.p[--b.len] = 0;
    return b.p;
}

// After `x=$(cmd)` alone, $? is cmd's status
static int last_status_after_subst(void) { return last_status; }

// ---- built-in commands ----

static int builtin_test(int argc, char **argv);

static const char *const builtin_names[] = {
    "help", "clear", "export", "unset", "set", "restart-kbd", "loadkeys", "exit", "cd", "pwd", "shift", "local",
    "return", "break", "continue", "source", ".", "eval", "read", "test", "[", "echo", "true", "false", ":",
    "type", "alias", "unalias", "wait", "umask", "exec", "history", "readonly", 0,
};
static const char *const *le_builtins(void) { return builtin_names; }

static int is_builtin(const char *c) {
    for (int i = 0; builtin_names[i]; i++)
        if (!strcmp(c, builtin_names[i])) return 1;
    return 0;
}

static void cmd_help(void) {
    printf("\n" ANSI_CYAN "Built-in commands:" ANSI_RESET "\n"
           "  cd [DIR], pwd, export NAME[=VALUE], unset NAME, set [-ex] [-- ARGS], local NAME[=V]\n"
           "  source FILE (or . FILE), eval, read [-r] [-p PROMPT] NAME..., test / [ ], echo\n"
           "  shift [N], return [N], break [N], continue [N], exit [N], true, false, :\n"
           "  type NAME, alias NAME=TEXT, unalias, umask, history, clear, help\n"
           "  loadkeys [LAYOUT]   keyboard layout (us, de, uk; none: list them)\n"
           "  restart-kbd         restart the keyboard driver\n"
           "\n" ANSI_CYAN "Programs in /bin (man NAME, or NAME --help):" ANSI_RESET "\n"
           "  files: ls cp mv rm mkdir rmdir touch ln stat du df find chmod realpath mktemp\n"
           "  text:  cat head tail wc sort uniq cut tr tee grep diff less edit (nano) nl paste\n"
           "         fold fmt join comm od printf seq tac expand base64 md5sum sha256sum\n"
           "  more:  tar gzip gunzip zcat xargs basename dirname env printenv date sleep yes\n"
           "         id whoami uname nproc expr man\n"
           "  system: ps top kill uptime free dmesg hostname reboot poweroff\n"
           "  tests: runtests systest fstest libctest threadtest objtest ipctest spawntest\n"
           "         drvtest smptest hello-std termtest\n"
           "\n" ANSI_CYAN "Shell (man shell):" ANSI_RESET "\n"
           "  a | b, a && b, a || b, a ; b, a &, < > >> 2> 2>&1 <<EOF, $VAR ${VAR:-x} $(cmd)\n"
           "  $((1+2)), *.txt, if/then/elif/else/fi, while/until/do/done, for x in ...; do ...; done,\n"
           "  case x in a) ...;; esac, name() { ...; }, scripts: shell FILE or #!/bin/sh\n"
           "  Editing: arrows, Home/End, Ctrl+A/E/K/U/W/L, Up/Down for history, Tab completes.\n"
           "  Shift+PgUp/PgDn scroll back. / and /tmp are in memory (lost at reboot).\n\n");
}

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

// read [-r] [-p PROMPT] NAME...: one line of standard input, split into
// the names (the last gets the rest; none: REPLY)
static int cmd_read(int argc, char **argv) {
    int raw = 0, i = 1;
    const char *prompt = 0;
    for (; i < argc && argv[i][0] == '-' && argv[i][1]; i++) {
        if (!strcmp(argv[i], "-r")) raw = 1;
        else if (!strcmp(argv[i], "-p") && i + 1 < argc) prompt = argv[++i];
        else if (!strcmp(argv[i], "--")) { i++; break; }
        else {
            fprintf(stderr, "read: unknown option %s\n", argv[i]);
            return 2;
        }
    }
    if (prompt) {
        fputs(prompt, stderr);
        fflush(stderr);
    }
    // A byte at a time: what follows the line stays for the next reader
    struct buf line = { 0 };
    puts_(&line, "");
    int got = 0, ended = 0;
    char c;
    while (read(0, &c, 1) == 1) {
        got = 1;
        if (c == '\n') { ended = 1; break; }
        if (c == '\\' && !raw) {
            if (read(0, &c, 1) != 1) break;
            if (c == '\n') continue;
        }
        putc_(&line, c);
    }
    if (!got) {
        free(line.p);
        return 1;
    }
    const char *ifs = get_var("IFS");
    if (!ifs) ifs = " \t\n";
    char *s = line.p;
    while (*s && strchr(ifs, *s)) s++;
    if (i >= argc) set_var("REPLY", line.p);
    for (; i < argc; i++) {
        if (!is_name(argv[i], strlen(argv[i]))) {
            fprintf(stderr, "read: not a valid name: %s\n", argv[i]);
            free(line.p);
            return 2;
        }
        size_t n;
        if (i == argc - 1) {
            n = strlen(s);
            while (n && strchr(ifs, s[n - 1])) n--;
        } else {
            n = strcspn(s, ifs);
        }
        char *v = xstrndup(s, n);
        set_var(argv[i], v);
        free(v);
        s += n;
        while (*s && strchr(ifs, *s)) s++;
    }
    free(line.p);
    return ended ? 0 : 1; // a last line without '\n' still sets the names
}

// echo [-n] [-e] ARGS
static int cmd_echo(int argc, char **argv) {
    int newline = 1, escapes = 0, i = 1;
    for (; i < argc && argv[i][0] == '-' && argv[i][1]; i++) {
        const char *o = argv[i] + 1;
        if (strspn(o, "neE") != strlen(o)) break;
        for (; *o; o++) {
            if (*o == 'n') newline = 0;
            else if (*o == 'e') escapes = 1;
            else escapes = 0;
        }
    }
    for (int first = 1; i < argc; i++, first = 0) {
        if (!first) putchar(' ');
        const char *s = argv[i];
        if (!escapes) {
            fputs(s, stdout);
            continue;
        }
        for (; *s; s++) {
            if (*s != '\\' || !s[1]) {
                putchar(*s);
                continue;
            }
            s++;
            switch (*s) {
            case 'n': putchar('\n'); break;
            case 't': putchar('\t'); break;
            case 'r': putchar('\r'); break;
            case 'a': putchar('\a'); break;
            case 'b': putchar('\b'); break;
            case 'e': putchar(033); break;
            case '\\': putchar('\\'); break;
            case 'c': fflush(stdout); return 0;
            case '0': {
                int v = 0, k = 0;
                while (k < 3 && s[1] >= '0' && s[1] <= '7') { v = v * 8 + (*++s - '0'); k++; }
                putchar(v);
                break;
            }
            default: putchar('\\'); putchar(*s);
            }
        }
    }
    if (newline) putchar('\n');
    return 0;
}

static int cmd_export(int argc, char **argv) {
    if (argc == 1 || (argc == 2 && !strcmp(argv[1], "-p"))) {
        for (int i = 0; i < nvars; i++)
            if (vars[i].exported) printf("export %s=\"%s\"\n", vars[i].name, vars[i].value);
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
        char *name = xstrndup(argv[i], n);
        if (eq) set_var(name, eq + 1);
        export_var(name);
        free(name);
    }
    return status;
}

static int cmd_set(int argc, char **argv) {
    if (argc == 1) {
        for (int i = 0; i < nvars; i++) printf("%s='%s'\n", vars[i].name, vars[i].value);
        return 0;
    }
    int i = 1, dashdash = 0;
    for (; i < argc; i++) {
        const char *a = argv[i];
        if (!strcmp(a, "--")) { i++; dashdash = 1; break; }
        if ((a[0] != '-' && a[0] != '+') || !a[1]) break;
        int on = a[0] == '-';
        if (!strcmp(a + 1, "o") && i + 1 < argc) {
            const char *o = argv[++i];
            if (!strcmp(o, "errexit")) opt_errexit = on;
            else if (!strcmp(o, "xtrace")) opt_xtrace = on;
            continue;
        }
        for (const char *o = a + 1; *o; o++) {
            if (*o == 'e') opt_errexit = on;
            else if (*o == 'x') opt_xtrace = on;
            else if (*o == 'u' || *o == 'f' || *o == 'v') {} // accepted, not supported
            else {
                fprintf(stderr, "set: unknown option %c%c\n", a[0], *o);
                return 2;
            }
        }
    }
    if (i < argc || dashdash) {
        // New positional parameters (kept; the old ones may be the caller's)
        int n = argc - i;
        char **v = calloc((size_t)n + 1, sizeof *v);
        for (int k = 0; k < n; k++) v[k] = xstrdup(argv[i + k]);
        pos_argv = v;
        pos_argc = n;
    }
    return 0;
}

static int cmd_type(int argc, char **argv) {
    int status = 0;
    for (int i = 1; i < argc; i++) {
        char path[PATH_MAX];
        if (find_alias(argv[i])) printf("%s is an alias for %s\n", argv[i], find_alias(argv[i]));
        else if (find_func(argv[i])) printf("%s is a function\n", argv[i]);
        else if (is_builtin(argv[i])) printf("%s is a shell built-in\n", argv[i]);
        else if (find_program(argv[i], path, sizeof path, 1) == 0) printf("%s is %s\n", argv[i], path);
        else {
            fprintf(stderr, "type: %s: not found\n", argv[i]);
            status = 1;
        }
    }
    return status;
}

static int cmd_alias(int argc, char **argv) {
    if (argc == 1) {
        for (int i = 0; i < naliases; i++) printf("alias %s='%s'\n", aliases[i].name, aliases[i].value);
        return 0;
    }
    int status = 0;
    for (int i = 1; i < argc; i++) {
        char *eq = strchr(argv[i], '=');
        if (!eq) {
            const char *v = find_alias(argv[i]);
            if (v) printf("alias %s='%s'\n", argv[i], v);
            else {
                fprintf(stderr, "alias: %s: not found\n", argv[i]);
                status = 1;
            }
            continue;
        }
        char *name = xstrndup(argv[i], (size_t)(eq - argv[i]));
        int k;
        for (k = 0; k < naliases; k++)
            if (!strcmp(aliases[k].name, name)) break;
        if (k == naliases) {
            if (naliases == 64) {
                free(name);
                return 1;
            }
            aliases[naliases].name = name;
            aliases[naliases++].value = 0;
        } else {
            free(name);
        }
        free(aliases[k].value);
        aliases[k].value = xstrdup(eq + 1);
    }
    return status;
}

static int cmd_source(int argc, char **argv) {
    if (argc < 2) {
        fprintf(stderr, "%s: FILE needed\n", argv[0]);
        return 2;
    }
    char path[PATH_MAX];
    const char *file = argv[1];
    if (!strchr(file, '/') && access(file, R_OK) < 0 && find_program(file, path, sizeof path, 1) == 0) file = path;
    int fd = open(file, O_RDONLY);
    if (fd < 0) {
        fprintf(stderr, "%s: %s: %s\n", argv[0], argv[1], strerror(errno));
        return 1;
    }
    struct buf b = { 0 };
    puts_(&b, "");
    char chunk[4096];
    ssize_t k;
    while ((k = read(fd, chunk, sizeof chunk)) > 0) put(&b, chunk, (size_t)k);
    close(fd);
    char **old_argv = pos_argv;
    int old_argc = pos_argc;
    if (argc > 2) {
        pos_argv = argv + 2;
        pos_argc = argc - 2;
    }
    func_depth++; // `return` leaves the file
    int st = run_string(b.p, argv[1]);
    func_depth--;
    if (returning) {
        returning = 0;
        st = last_status;
    }
    pos_argv = old_argv;
    pos_argc = old_argc;
    free(b.p);
    return st;
}

static struct le editor;

static int run_builtin(int argc, char **argv, int *status) {
    const char *c = argv[0];
    *status = 0;
    if (!strcmp(c, "help")) cmd_help();
    else if (!strcmp(c, "clear")) printf("\033[2J\033[H");
    else if (!strcmp(c, "true") || !strcmp(c, ":")) *status = 0;
    else if (!strcmp(c, "false")) *status = 1;
    else if (!strcmp(c, "echo")) *status = cmd_echo(argc, argv);
    else if (!strcmp(c, "test") || !strcmp(c, "[")) *status = builtin_test(argc, argv);
    else if (!strcmp(c, "export") || !strcmp(c, "readonly")) *status = cmd_export(argc, argv);
    else if (!strcmp(c, "unset")) {
        int functions = 0;
        for (int i = 1; i < argc; i++) {
            if (!strcmp(argv[i], "-v")) continue;
            if (!strcmp(argv[i], "-f")) { functions = 1; continue; }
            if (functions) {
                for (int j = 0; j < nfuncs; j++)
                    if (!strcmp(funcs[j].name, argv[i])) {
                        free(funcs[j].name);
                        funcs[j] = funcs[--nfuncs];
                        break;
                    }
            } else {
                unset_var(argv[i]);
            }
        }
    } else if (!strcmp(c, "set")) *status = cmd_set(argc, argv);
    else if (!strcmp(c, "local")) {
        if (!func_depth) {
            fprintf(stderr, "local: only in a function\n");
            *status = 1;
        } else {
            for (int i = 1; i < argc; i++) {
                char *eq = strchr(argv[i], '=');
                char *name = eq ? xstrndup(argv[i], (size_t)(eq - argv[i])) : xstrdup(argv[i]);
                make_local(name);
                if (eq) set_var(name, eq + 1);
                free(name);
            }
        }
    } else if (!strcmp(c, "shift")) {
        int k = argc > 1 ? atoi(argv[1]) : 1;
        if (k < 0 || k > pos_argc) *status = 1;
        else {
            pos_argv += k;
            pos_argc -= k;
        }
    } else if (!strcmp(c, "return")) {
        *status = argc > 1 ? atoi(argv[1]) & 0xFF : last_status;
        last_status = *status;
        if (func_depth) {
            returning = 1;
        } else { // in a script: leave it
            exiting = 1;
            exit_code = *status;
        }
    } else if (!strcmp(c, "break") || !strcmp(c, "continue")) {
        int k = argc > 1 ? atoi(argv[1]) : 1;
        if (k < 1) k = 1;
        if (!loop_depth) {
            fprintf(stderr, "%s: only in a loop\n", c);
            *status = 1;
        } else {
            if (k > loop_depth) k = loop_depth;
            if (c[0] == 'b') breaking = k;
            else continuing = k;
        }
    } else if (!strcmp(c, "source") || !strcmp(c, ".")) *status = cmd_source(argc, argv);
    else if (!strcmp(c, "eval")) {
        struct buf b = { 0 };
        puts_(&b, "");
        for (int i = 1; i < argc; i++) {
            if (i > 1) putc_(&b, ' ');
            puts_(&b, argv[i]);
        }
        *status = run_string(b.p, 0);
        free(b.p);
    } else if (!strcmp(c, "read")) *status = cmd_read(argc, argv);
    else if (!strcmp(c, "type")) *status = cmd_type(argc, argv);
    else if (!strcmp(c, "alias")) *status = cmd_alias(argc, argv);
    else if (!strcmp(c, "unalias")) {
        for (int i = 1; i < argc; i++)
            for (int k = 0; k < naliases; k++)
                if (!strcmp(argv[i], "-a") || !strcmp(aliases[k].name, argv[i])) {
                    free(aliases[k].name);
                    free(aliases[k].value);
                    aliases[k--] = aliases[--naliases];
                }
    } else if (!strcmp(c, "wait")) {
        *status = 0; // programs in the background are not followed after they start
    } else if (!strcmp(c, "umask")) {
        if (argc > 1) umask((mode_t)strtol(argv[1], 0, 8));
        else {
            mode_t m = umask(0);
            umask(m);
            printf("%04o\n", (unsigned)m);
        }
    } else if (!strcmp(c, "history")) {
        for (int i = 0; i < editor.nhist; i++) printf("%5d  %s\n", i + 1, editor.history[i]);
    } else if (!strcmp(c, "exec")) {
        // No exec: run the program, then leave with its status
        if (argc > 1) {
            struct node tmp = { 0 };
            tmp.type = N_SIMPLE;
            tmp.words = argv + 1;
            tmp.nwords = argc - 1;
            *status = exec_simple(&tmp, 0);
            exiting = 1;
            exit_code = *status;
        }
    } else if (!strcmp(c, "restart-kbd")) {
        if (!init_ctl || rx_channel_write(init_ctl, 0, "restart-kbd", 11, 0, 0) != RX_OK) {
            fprintf(stderr, "restart-kbd: init is not listening\n");
            *status = 1;
        }
    } else if (!strcmp(c, "loadkeys")) {
        *status = cmd_loadkeys(argc, argv);
    } else if (!strcmp(c, "cd")) {
        const char *dir = argc > 1 ? argv[1] : get_var("HOME");
        char old[PATH_MAX];
        if (!getcwd(old, sizeof old)) old[0] = 0;
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
            char now[PATH_MAX];
            set_var("OLDPWD", old);
            export_var("OLDPWD");
            if (getcwd(now, sizeof now)) {
                set_var("PWD", now);
                export_var("PWD");
            }
        }
    } else if (!strcmp(c, "pwd")) {
        char cwd[PATH_MAX];
        printf("%s\n", getcwd(cwd, sizeof cwd) ? cwd : "?");
    } else if (!strcmp(c, "exit")) {
        exiting = 1;
        exit_code = argc > 1 ? atoi(argv[1]) & 0xFF : last_status;
        *status = exit_code;
    } else {
        return 0;
    }
    return 1;
}

// ---- test / [ ----

static int t_unary(const char *op, const char *a) {
    struct stat st;
    switch (op[1]) {
    case 'z': return !*a;
    case 'n': return *a != 0;
    case 'e': return stat(a, &st) == 0;
    case 'f': return stat(a, &st) == 0 && S_ISREG(st.st_mode);
    case 'd': return stat(a, &st) == 0 && S_ISDIR(st.st_mode);
    case 'c': return stat(a, &st) == 0 && S_ISCHR(st.st_mode);
    case 's': return stat(a, &st) == 0 && st.st_size > 0;
    case 'r': return access(a, R_OK) == 0;
    case 'w': return access(a, W_OK) == 0;
    case 'x': return access(a, X_OK) == 0;
    case 'h': case 'L': return 0; // no symbolic links
    case 't': return isatty(atoi(a));
    }
    return 0;
}

static int t_is_unary(const char *s) { return s[0] == '-' && s[1] && !s[2] && strchr("znefdcsrwxhLt", s[1]); }

static int t_is_binary(const char *s) {
    static const char *const ops[] = { "=", "==", "!=", "-eq", "-ne", "-lt", "-le", "-gt", "-ge", "<", ">", "-nt", "-ot", 0 };
    for (int i = 0; ops[i]; i++)
        if (!strcmp(s, ops[i])) return 1;
    return 0;
}

static int t_num(const char *s, long *v) {
    char *e;
    while (*s == ' ') s++;
    *v = strtol(s, &e, 10);
    while (*e == ' ') e++;
    return *s && !*e;
}

static int t_binary(const char *a, const char *op, const char *b, int *err) {
    if (!strcmp(op, "=") || !strcmp(op, "==")) return !strcmp(a, b);
    if (!strcmp(op, "!=")) return strcmp(a, b) != 0;
    if (!strcmp(op, "<")) return strcmp(a, b) < 0;
    if (!strcmp(op, ">")) return strcmp(a, b) > 0;
    if (!strcmp(op, "-nt") || !strcmp(op, "-ot")) {
        struct stat x, y;
        int hx = stat(a, &x) == 0, hy = stat(b, &y) == 0;
        long tx = hx ? x.st_mtim.tv_sec * 1000000000L + x.st_mtim.tv_nsec : 0;
        long ty = hy ? y.st_mtim.tv_sec * 1000000000L + y.st_mtim.tv_nsec : 0;
        return op[1] == 'n' ? (hx && (!hy || tx > ty)) : (hy && (!hx || tx < ty));
    }
    long x, y;
    if (!t_num(a, &x) || !t_num(b, &y)) {
        fprintf(stderr, "test: integer expected: %s\n", t_num(a, &x) ? b : a);
        *err = 1;
        return 0;
    }
    if (!strcmp(op, "-eq")) return x == y;
    if (!strcmp(op, "-ne")) return x != y;
    if (!strcmp(op, "-lt")) return x < y;
    if (!strcmp(op, "-le")) return x <= y;
    if (!strcmp(op, "-gt")) return x > y;
    return x >= y;
}

struct tst { char **v; int n, i, err; };

static int t_or(struct tst *t);

static int t_prim(struct tst *t) {
    if (t->i >= t->n) {
        t->err = 1;
        return 0;
    }
    char *a = t->v[t->i];
    if (!strcmp(a, "!")) {
        t->i++;
        return !t_prim(t);
    }
    if (!strcmp(a, "(")) {
        t->i++;
        int r = t_or(t);
        if (t->i < t->n && !strcmp(t->v[t->i], ")")) t->i++;
        else t->err = 1;
        return r;
    }
    if (t->i + 2 < t->n + 1 && t->i + 1 < t->n && t_is_binary(t->v[t->i + 1]) && t->i + 2 < t->n) {
        int r = t_binary(a, t->v[t->i + 1], t->v[t->i + 2], &t->err);
        t->i += 3;
        return r;
    }
    if (t_is_unary(a) && t->i + 1 < t->n) {
        int r = t_unary(a, t->v[t->i + 1]);
        t->i += 2;
        return r;
    }
    t->i++;
    return *a != 0;
}

static int t_and(struct tst *t) {
    int r = t_prim(t);
    while (t->i < t->n && !strcmp(t->v[t->i], "-a")) {
        t->i++;
        int s = t_prim(t);
        r = r && s;
    }
    return r;
}

static int t_or(struct tst *t) {
    int r = t_and(t);
    while (t->i < t->n && !strcmp(t->v[t->i], "-o")) {
        t->i++;
        int s = t_and(t);
        r = r || s;
    }
    return r;
}

static int builtin_test(int argc, char **argv) {
    int n = argc - 1;
    if (!strcmp(argv[0], "[")) {
        if (n < 1 || strcmp(argv[argc - 1], "]")) {
            fprintf(stderr, "[: missing ]\n");
            return 2;
        }
        n--;
    }
    struct tst t = { argv + 1, n, 0, 0 };
    if (n == 0) return 1;
    int r;
    // POSIX: the meaning of 1 to 3 arguments goes by their number
    if (n == 1) r = *t.v[0] != 0;
    else if (n == 2 && !strcmp(t.v[0], "!")) r = !*t.v[1];
    else if (n == 2 && t_is_unary(t.v[0])) r = t_unary(t.v[0], t.v[1]);
    else if (n == 3 && t_is_binary(t.v[1])) r = t_binary(t.v[0], t.v[1], t.v[2], &t.err);
    else if (n == 3 && !strcmp(t.v[0], "!")) {
        struct tst u = { t.v + 1, 2, 0, 0 };
        r = !t_or(&u);
        t.err = u.err;
    } else {
        r = t_or(&t);
        if (t.i < t.n) {
            t.err = 1;
            fprintf(stderr, "test: bad expression\n");
        }
    }
    if (t.err) return 2;
    return r ? 0 : 1;
}

// ---- input ----

static int read_line(char *buf, size_t cap) {
    size_t len = 0;
    for (;;) {
        uint32_t observed = 0;
        if (console) {
            rx_object_wait_one(console, RX_CHANNEL_READABLE | RX_PEER_CLOSED | RX_CONSOLE_SIGNAL_INTERRUPT,
                               RX_TIME_INFINITE, &observed);
            if (observed & RX_CONSOLE_SIGNAL_INTERRUPT) {
                rx_object_signal(console, RX_CONSOLE_SIGNAL_INTERRUPT, 0);
                return -1;
            }
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

static char *slurp(int fd) {
    struct buf b = { 0 };
    puts_(&b, "");
    char chunk[4096];
    ssize_t k;
    while ((k = read(fd, chunk, sizeof chunk)) > 0) put(&b, chunk, (size_t)k);
    return b.p;
}

static int interactive_loop(void) {
    static char line[LINE_MAX_LEN];
    show_welcome();
    editor.console = console;
    struct buf src = { 0 };
    for (;;) {
        fflush(stdout);
        char cwd[PATH_MAX], prompt[PATH_MAX + 64];
        if (src.len) {
            strcpy(prompt, "> ");
        } else {
            if (!getcwd(cwd, sizeof cwd)) strcpy(cwd, "?");
            snprintf(prompt, sizeof prompt, ANSI_MAGENTA "rustux" ANSI_RESET ":" ANSI_CYAN "%s" ANSI_RESET " > ", cwd);
        }
        int n;
        if (le_raw()) {
            n = le_read(&editor, prompt, line, sizeof line);
            le_cooked_mode();
        } else {
            fputs(prompt, stdout);
            fflush(stdout);
            n = read_line(line, sizeof line);
        }
        if (n < 0) { // Ctrl+C: drop the unfinished command
            src.len = 0;
            last_status = 130;
            continue;
        }
        if (n == 0) {
            if (src.len) {
                fprintf(stderr, "shell: unexpected end of input\n");
                src.len = 0;
                continue;
            }
            if (rx_object_wait_one(console, RX_PEER_CLOSED, 0, 0) != RX_OK) {
                printf("\nUse \"exit\" to leave the shell.\n");
                continue;
            }
            return 0; // the console is gone
        }
        put(&src, line, (size_t)n);
        int incomplete = 0;
        struct node *tree = parse_program(src.p, &incomplete);
        if (!tree && incomplete) continue; // more lines needed
        src.len = 0;
        if (!tree) {
            last_status = 2;
            continue;
        }
        interrupted = 0;
        exec_node(tree);
        free_node(tree);
        breaking = continuing = returning = 0;
        if (interrupted) {
            interrupted = 0;
            last_status = 130;
        }
        if (exiting) {
            printf(ANSI_GREEN "Exiting shell." ANSI_RESET "\n");
            return exit_code;
        }
    }
}

static void usage(void) {
    fprintf(stderr, "usage: shell [-ex] [-c COMMANDS [NAME [ARGS]...] | FILE [ARGS]...]\n");
}

int main(int argc, char **argv) {
    init_ctl = rx_take_startup_handle(RX_PA_USER0);
    power = rx_take_startup_handle(RX_PA_POWER_RESOURCE);
    if (rx_fd_handle(0) && isatty(0)) rx_handle_duplicate(rx_fd_handle(0), RX_RIGHT_SAME_RIGHTS, &console);
    import_environment();
    if (!get_var("PATH")) {
        set_var("PATH", "/bin");
        export_var("PATH");
    }
    const char *command = 0;
    int i = 1;
    for (; i < argc && argv[i][0] == '-' && argv[i][1]; i++) {
        if (!strcmp(argv[i], "--")) { i++; break; }
        if (!strcmp(argv[i], "--help")) {
            usage();
            printf("Without FILE or -c: interactive. See man shell.\n");
            return 0;
        }
        for (const char *o = argv[i] + 1; *o; o++) {
            if (*o == 'c') command = "";
            else if (*o == 'e') opt_errexit = 1;
            else if (*o == 'x') opt_xtrace = 1;
            else if (*o == 'i' || *o == 's' || *o == 'l') {}
            else {
                usage();
                return 2;
            }
        }
    }
    int status;
    if (command) {
        if (i >= argc) {
            usage();
            return 2;
        }
        command = argv[i++];
        if (i < argc) script_name = argv[i++];
        pos_argv = argv + i;
        pos_argc = argc - i;
        status = run_string(command, 0);
    } else if (i < argc) {
        // A script file (its #! line is a comment to us)
        script_name = argv[i];
        int fd = open(argv[i], O_RDONLY);
        if (fd < 0) {
            fprintf(stderr, "shell: %s: %s\n", argv[i], strerror(errno));
            return 127;
        }
        char *src = slurp(fd);
        close(fd);
        pos_argv = argv + i + 1;
        pos_argc = argc - i - 1;
        status = run_string(src, script_name);
        free(src);
    } else if (!console) {
        // Commands from a pipe or a file on standard input
        char *src = slurp(0);
        status = run_string(src, "stdin");
        free(src);
    } else {
        interactive = 1;
        return interactive_loop();
    }
    fflush(stdout);
    return exiting ? exit_code : status;
}
