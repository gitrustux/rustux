// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// appctl: start an installed app in its sandbox (FoxDen step FD2,
// docs/FOXDEN.md "The sandbox")
//
//   appctl run ID [ARG...]     start /apps/ID's Exec, sandboxed; waits, and
//                              exits with its code
//   appctl open ID FILE...     the same, with those files (and nothing beside
//                              them) in its view, given as its arguments:
//                              Files opening a file with an app (step FD6b)
//   appctl view ID             the paths it would see, one per line (MODE PATH)
//   appctl permissions ID      what it asks for and what it gets
//
// An app sees a root of its own (an fsd view, docs/FS.md "Views"): its own
// folder and /usr read-only, its data folders (~/.local/share/apps/ID,
// ~/.config/apps/ID, ~/.cache/apps/ID) read-write, a few files of /etc,
// /dev/null and /dev/zero, and the services every app needs (/svc/window,
// /svc/log); its permissions add the rest. It runs in a job of its own
// with the sandbox policy (no calls that reach the system without a
// handle). The user narrows an app's permissions in
// ~/.config/apps/permissions ("ID: network files:documents"; "ID: -" for
// none); the manifest's are the most it gets.
//
// While the app runs, appctl is its file chooser (FoxDen step FD6,
// <rustux/portal.h>): the app gets a channel (RX_PA_PORTAL) on which it
// asks to open or save a file; appctl shows the chooser (/bin/chooser, a
// window of the session's, as the user), opens the file the user picks
// and hands the app that file's connection alone.
//
// appctl open (FoxDen step FD6b) is how Files opens a file with an app:
// each FILE the user may read is added to the view at its own path, read
// only, or read-write when the user may write it; the app gets the paths
// as its arguments, so it opens them as it would anywhere. The file
// alone: its folder lists only it.

#include <errno.h>
#include <fcntl.h>
#include <pwd.h>
#include <rustux/process.h>
#include <rustux/syscalls.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mount.h>
#include <sys/stat.h>
#include <unistd.h>

#define MAXV 48

static const char *ID;
static char manifest[8192];

static int valid_id(const char *s) {
    int parts = 1, len = 0;
    if (!*s || *s == '.') return 0;
    for (const char *c = s; *c; c++) {
        if (*c == '.') {
            if (len == 0 || c[1] == 0) return 0;
            parts++;
            len = 0;
        } else if ((*c >= 'a' && *c <= 'z') || (*c >= '0' && *c <= '9') || *c == '-') {
            len++;
        } else {
            return 0;
        }
    }
    return parts >= 2 && strlen(s) < 200;
}

// The value of field `key` in the manifest (copied to out), or 0
static const char *field(const char *key, char *out, size_t cap) {
    size_t kl = strlen(key);
    for (const char *l = manifest; *l;) {
        const char *e = strchr(l, '\n');
        size_t n = e ? (size_t)(e - l) : strlen(l);
        if (n > kl && !strncmp(l, key, kl) && l[kl] == ':') {
            const char *v = l + kl + 1;
            size_t vl = n - kl - 1;
            while (vl && (*v == ' ' || *v == '\t')) v++, vl--;
            while (vl && (v[vl - 1] == ' ' || v[vl - 1] == '\r' || v[vl - 1] == '\t')) vl--;
            if (vl >= cap) vl = cap - 1;
            memcpy(out, v, vl);
            out[vl] = 0;
            return out;
        }
        l = e ? e + 1 : l + n;
    }
    return 0;
}

static int load_manifest(void) {
    char p[300];
    snprintf(p, sizeof p, "/apps/%s/manifest", ID);
    FILE *f = fopen(p, "r");
    if (!f) {
        fprintf(stderr, "appctl: %s: not installed (no %s)\n", ID, p);
        return -1;
    }
    size_t n = fread(manifest, 1, sizeof manifest - 1, f);
    manifest[n] = 0;
    fclose(f);
    char v[256];
    if (!field("Id", v, sizeof v) || strcmp(v, ID)) {
        fprintf(stderr, "appctl: %s: its manifest names another app\n", ID);
        return -1;
    }
    return 0;
}

// ---- permissions ----

static const char *KNOWN[] = { "network", "files:documents", "files:pictures", "files:music", "files:videos",
                               "files:downloads", "files:home", "removable", "audio", "camera", "location",
                               "system", 0 };

static int has_word(const char *list, const char *w) {
    size_t wl = strlen(w);
    for (const char *c = list; *c;) {
        while (*c == ' ' || *c == '\t' || *c == ',') c++;
        size_t n = 0;
        while (c[n] && c[n] != ' ' && c[n] != '\t' && c[n] != ',') n++;
        if (n == wl && !strncmp(c, w, n)) return 1;
        c += n;
    }
    return 0;
}

static char asked[1024], user_line[1024];
static int user_set;   // the user has a line for this app
static int granted[16];

static void permissions(const char *home) {
    if (!field("Permissions", asked, sizeof asked)) asked[0] = 0;
    char p[600], line[1200];
    snprintf(p, sizeof p, "%s/.config/apps/permissions", home);
    FILE *f = fopen(p, "r");
    size_t il = strlen(ID);
    if (f) {
        while (fgets(line, sizeof line, f)) {
            char *l = line;
            while (*l == ' ' || *l == '\t') l++;
            if (*l == '#' || strncmp(l, ID, il) || l[il] != ':') continue;
            snprintf(user_line, sizeof user_line, "%s", l + il + 1);
            user_line[strcspn(user_line, "\r\n")] = 0;
            user_set = 1;
        }
        fclose(f);
    }
    for (int k = 0; KNOWN[k]; k++) {
        granted[k] = has_word(asked, KNOWN[k]) && (!user_set || has_word(user_line, KNOWN[k]));
        // system: only apps from the system's own sources (step FD3); none yet
        if (!strcmp(KNOWN[k], "system")) granted[k] = 0;
    }
}

static int perm(const char *name) {
    for (int k = 0; KNOWN[k]; k++)
        if (!strcmp(KNOWN[k], name)) return granted[k];
    return 0;
}

// ---- the view ----

static int vn;
static int vmode[MAXV];
static char *vpath[MAXV];

static void add(int mode, const char *fmt, const char *a) {
    if (vn == MAXV) return;
    char p[600];
    snprintf(p, sizeof p, fmt, a);
    vmode[vn] = mode;
    vpath[vn++] = strdup(p);
}

static void mkdirs(const char *path) {
    char p[600];
    snprintf(p, sizeof p, "%s", path);
    for (char *c = p + 1; *c; c++) {
        if (*c != '/') continue;
        *c = 0;
        mkdir(p, 0700);
        *c = '/';
    }
    mkdir(p, 0700);
}

static void build_view(const char *home) {
    static const char *etc[] = { "passwd", "group", "timezone", "localtime", "os-release", "hostname", 0 };
    add(RX_FS_VIEW_RO, "/apps/%s", ID);
    add(RX_FS_VIEW_RO, "/usr", "");
    for (int k = 0; etc[k]; k++) add(RX_FS_VIEW_RO, "/etc/%s", etc[k]);
    add(RX_FS_VIEW_OPEN, "/dev/null", "");
    add(RX_FS_VIEW_OPEN, "/dev/zero", "");
    add(RX_FS_VIEW_OPEN, "/svc/window", "");
    add(RX_FS_VIEW_OPEN, "/svc/log", "");
    static const char *data[] = { ".local/share/apps", ".config/apps", ".cache/apps", 0 };
    for (int k = 0; data[k]; k++) {
        char p[600];
        snprintf(p, sizeof p, "%s/%s/%s", home, data[k], ID);
        add(RX_FS_VIEW_RW, "%s", p);
    }
    if (perm("network")) {
        add(RX_FS_VIEW_OPEN, "/svc/netsock", "");
        add(RX_FS_VIEW_RO, "/etc/resolv.conf", "");
        add(RX_FS_VIEW_RO, "/etc/hosts", "");
    }
    static const char *dirs[][2] = { { "files:documents", "Documents" }, { "files:pictures", "Pictures" },
                                     { "files:music", "Music" }, { "files:videos", "Videos" },
                                     { "files:downloads", "Downloads" } };
    if (perm("files:home")) {
        add(RX_FS_VIEW_RW, "%s", home);
    } else {
        for (int k = 0; k < 5; k++) {
            if (!perm(dirs[k][0])) continue;
            char p[600];
            snprintf(p, sizeof p, "%s/%s", home, dirs[k][1]);
            add(RX_FS_VIEW_RW, "%s", p);
        }
    }
    if (perm("removable")) add(RX_FS_VIEW_RW, "/media", "");
    // audio, camera, location: no services for them yet
}

// FILE (from the current folder if relative) into the view, at its own
// path: read-write if the user may write it, else read only. 0, or -1
// (said why)
static int add_file(const char *arg, char *abs, size_t cap) {
    if (arg[0] == '/') {
        snprintf(abs, cap, "%s", arg);
    } else {
        char cwd[512];
        if (!getcwd(cwd, sizeof cwd)) {
            fprintf(stderr, "appctl: %s: where is it? (no current folder)\n", arg);
            return -1;
        }
        snprintf(abs, cap, "%s/%s", strcmp(cwd, "/") ? cwd : "", arg);
    }
    size_t n = strlen(abs);
    if (strstr(abs, "/./") || strstr(abs, "/../") || strstr(abs, "//") || (n >= 2 && !strcmp(abs + n - 2, "/.")) ||
        (n >= 3 && !strcmp(abs + n - 3, "/..")) || abs[n - 1] == '/') {
        fprintf(stderr, "appctl: %s: a file's path, without . or ..\n", abs);
        return -1;
    }
    struct stat st;
    if (stat(abs, &st) < 0) {
        fprintf(stderr, "appctl: %s: %s\n", abs, strerror(errno));
        return -1;
    }
    if (!S_ISREG(st.st_mode)) {
        fprintf(stderr, "appctl: %s: not a file\n", abs);
        return -1;
    }
    if (access(abs, R_OK) < 0) {
        fprintf(stderr, "appctl: %s: %s\n", abs, strerror(errno));
        return -1;
    }
    if (vn == MAXV) {
        fprintf(stderr, "appctl: %s: too many files\n", abs);
        return -1;
    }
    add(access(abs, W_OK) == 0 ? RX_FS_VIEW_RW : RX_FS_VIEW_RO, "%s", abs);
    return 0;
}

static const char *mode_name(int m) { return m == RX_FS_VIEW_RO ? "ro" : m == RX_FS_VIEW_RW ? "rw" : "open"; }

// ---- running it ----

static float user_scale(const char *home) {
    const char *e = getenv("RUSTICA_SCALE");
    if (e && *e) return (float)atof(e);
    char p[600], line[256];
    snprintf(p, sizeof p, "%s/.config/rustica.conf", home);
    FILE *f = fopen(p, "r");
    float s = 0;
    if (!f) return 0;
    while (fgets(line, sizeof line, f))
        if (!strncmp(line, "scale=", 6)) s = (float)atof(line + 6);
    fclose(f);
    return s;
}

// ---- the file chooser (FoxDen step FD6) ----

extern char **environ;

// A request of the app's on its chooser channel: "open\nTITLE\nPATTERNS"
// or "save\nTITLE\nNAME". Answered "ok NAME" with the file's
// connection, "cancel", or "error ERRNO MESSAGE".
static void portal_request(rx_handle_t ch, const char *home, const char *app_name) {
    char req[1100];
    rx_handle_t hs[4];
    uint32_t got = 0, nh = 0;
    if (rx_channel_read(ch, 0, req, hs, sizeof req - 1, 4, &got, &nh) != RX_OK) return;
    for (uint32_t i = 0; i < nh; i++) rx_handle_close(hs[i]);
    req[got] = 0;
    char *verb = req, *title = strchr(req, '\n'), *extra = 0;
    if (title) {
        *title++ = 0;
        extra = strchr(title, '\n');
        if (extra) *extra++ = 0;
    }
    int save = !strcmp(verb, "save");
    if (!title || (!save && strcmp(verb, "open"))) {
        const char *e = "error 22 open or save";
        rx_channel_write(ch, 0, e, (uint32_t)strlen(e), 0, 0);
        return;
    }
    // Where the chooser writes its answer: a folder the app cannot see
    char dir[600], out[700];
    snprintf(dir, sizeof dir, "%s/.cache/appctl", home);
    mkdirs(dir);
    snprintf(out, sizeof out, "%s/choice-%s", dir, ID);
    unlink(out);
    char *argv[16];
    int n = 0;
    argv[n++] = "/bin/chooser";
    argv[n++] = save ? "save" : "open";
    argv[n++] = "--app";
    argv[n++] = (char *)app_name;
    argv[n++] = "--title";
    argv[n++] = title;
    argv[n++] = "--out";
    argv[n++] = out;
    if (extra && *extra) {
        argv[n++] = save ? "--name" : "--patterns";
        argv[n++] = extra;
    }
    argv[n] = 0;
    // Our environment, but the window where the compositor puts it
    char *envp[64];
    int ne = 0;
    for (char **e = environ; *e && ne < 63; e++)
        if (strncmp(*e, "RUSTICA_WINDOW_AT=", 18)) envp[ne++] = *e;
    envp[ne] = 0;
    rx_handle_t proc = 0;
    char msg[700];
    // (in our namespace, the user's, not the app's view)
    if (rx_spawn("/bin/chooser", argv, envp, RX_SPAWN_CLONE_ALL, 0, 0, &proc) != RX_OK) {
        snprintf(msg, sizeof msg, "error %d the file chooser did not start", ENOENT);
        rx_channel_write(ch, 0, msg, (uint32_t)strlen(msg), 0, 0);
        return;
    }
    int64_t code = rx_wait_process(proc);
    char path[600] = "";
    FILE *f = code == 0 ? fopen(out, "r") : 0;
    if (f) {
        if (!fgets(path, sizeof path, f)) path[0] = 0;
        fclose(f);
        path[strcspn(path, "\n")] = 0;
    }
    unlink(out);
    if (!path[0]) {
        printf("appctl: %s: the user said no (%s)\n", ID, verb);
        fflush(stdout);
        rx_channel_write(ch, 0, "cancel", 6, 0, 0);
        return;
    }
    // The file, opened as the user, handed over alone
    int fd = save ? open(path, O_WRONLY | O_CREAT | O_TRUNC, 0644) : open(path, O_RDONLY);
    struct stat st;
    if (fd >= 0 && (fstat(fd, &st) < 0 || !S_ISREG(st.st_mode))) {
        close(fd);
        fd = -1;
        errno = EISDIR;
    }
    if (fd < 0) {
        snprintf(msg, sizeof msg, "error %d %s: %s", errno, path, strerror(errno));
        rx_channel_write(ch, 0, msg, (uint32_t)strlen(msg), 0, 0);
        return;
    }
    rx_handle_t h = 0;
    int r = rx_fd_clone_handle(fd, &h);
    close(fd);
    if (r < 0) {
        snprintf(msg, sizeof msg, "error %d %s", -r, strerror(-r));
        rx_channel_write(ch, 0, msg, (uint32_t)strlen(msg), 0, 0);
        return;
    }
    const char *base = strrchr(path, '/') ? strrchr(path, '/') + 1 : path;
    snprintf(msg, sizeof msg, "ok %s", base);
    if (rx_channel_write(ch, 0, msg, (uint32_t)strlen(msg), &h, 1) != RX_OK) rx_handle_close(h);
    printf("appctl: %s: the user chose %s (%s)\n", ID, path, verb);
    fflush(stdout);
}

static int run(int argc, char **argv, const struct passwd *pw) {
    const char *home = pw->pw_dir;
    char exec[512];
    if (!field("Exec", exec, sizeof exec) || !exec[0]) {
        fprintf(stderr, "appctl: %s: no Exec in its manifest\n", ID);
        return 1;
    }
    // Exec: a program in the folder, then its arguments
    char *args[64];
    int na = 0;
    char *save;
    for (char *t = strtok_r(exec, " \t", &save); t && na < 40; t = strtok_r(0, " \t", &save)) args[na++] = t;
    if (args[0][0] == '/' || strstr(args[0], "..")) {
        fprintf(stderr, "appctl: %s: Exec must be inside its folder\n", ID);
        return 1;
    }
    char prog[600];
    snprintf(prog, sizeof prog, "/apps/%s/%s", ID, args[0]);
    for (int k = 0; k < argc && na < 63; k++) args[na++] = argv[k];
    args[0] = prog;
    args[na] = 0;

    // Its data folders, made if new
    static const char *data[] = { ".local/share/apps", ".config/apps", ".cache/apps", 0 };
    char dirs[3][600];
    for (int k = 0; data[k]; k++) {
        snprintf(dirs[k], sizeof dirs[k], "%s/%s/%s", home, data[k], ID);
        mkdirs(dirs[k]);
    }
    char tmp[620];
    snprintf(tmp, sizeof tmp, "%s/tmp", dirs[2]);
    mkdir(tmp, 0700);

    // The environment: a few of ours, and its own
    static char envs[24][700];
    char *envp[25];
    int ne = 0;
#define ENV(...) (snprintf(envs[ne], sizeof envs[ne], __VA_ARGS__), envp[ne] = envs[ne], ne++)
    ENV("HOME=%s", home);
    ENV("USER=%s", pw->pw_name);
    ENV("LOGNAME=%s", pw->pw_name);
    ENV("PATH=/apps/%s/bin", ID);
    ENV("PWD=%s", dirs[0]);
    ENV("APP_ID=%s", ID);
    ENV("XDG_DATA_HOME=%s", dirs[0]);
    ENV("XDG_CONFIG_HOME=%s", dirs[1]);
    ENV("XDG_CACHE_HOME=%s", dirs[2]);
    ENV("TMPDIR=%s", tmp);
    float sc = user_scale(home);
    if (sc >= 1 && sc <= 4) ENV("RUSTICA_SCALE=%d", (int)(sc + 0.5f));
    static const char *pass[] = { "RUSTICA_WINDOW_AT", "TZ", "LANG", "TERM", 0 };
    for (int k = 0; pass[k]; k++) {
        const char *v = getenv(pass[k]);
        if (v) ENV("%s=%s", pass[k], v);
    }
    envp[ne] = 0;

    rx_handle_t job = 0, ns = 0, proc = 0;
    rx_status_t s;
    int r = rx_fs_view(vn, vmode, (const char *const *)vpath, &ns);
    if (r < 0) {
        fprintf(stderr, "appctl: %s: its view: %s\n", ID, strerror(-r));
        return 1;
    }
    if ((s = rx_job_create(rx_job_default, 0, &job)) != RX_OK || (s = rx_job_set_policy(job, RX_JOB_SANDBOX)) != RX_OK) {
        fprintf(stderr, "appctl: %s: its job: %s\n", ID, strerror(rx_status_to_errno(s)));
        return 1;
    }
    // Its file chooser's channel: the other end stays here
    rx_handle_t portal = 0, portal_app = 0;
    if (rx_channel_create(0, &portal, &portal_app) != RX_OK) {
        fprintf(stderr, "appctl: %s: no channel for its file chooser\n", ID);
        return 1;
    }
    rx_spawn_handle_t hs[2] = { { RX_PA_HND(RX_PA_NS_DIR, 0), ns }, { RX_PA_HND(RX_PA_PORTAL, 0), portal_app } };
    s = rx_spawn_etc(job, prog, args, envp, RX_SPAWN_CLONE_STDIO, hs, 2, &proc);
    if (s != RX_OK) {
        fprintf(stderr, "appctl: %s: %s: %s\n", ID, prog, strerror(rx_status_to_errno(s)));
        return 1;
    }
    printf("appctl: started %s (sandboxed%s%s)\n", ID, asked[0] ? "; asked for: " : "", asked);
    fflush(stdout);
    // Its requests until it ends
    char app_name[256];
    if (!field("Name", app_name, sizeof app_name)) snprintf(app_name, sizeof app_name, "%s", ID);
    for (;;) {
        rx_wait_item_t w[2] = { { proc, RX_PROCESS_TERMINATED, 0 }, { portal, RX_CHANNEL_READABLE | RX_PEER_CLOSED, 0 } };
        rx_object_wait_many(w, portal ? 2 : 1, RX_TIME_INFINITE);
        if (w[0].pending & RX_PROCESS_TERMINATED) break;
        if (portal && (w[1].pending & RX_CHANNEL_READABLE)) portal_request(portal, home, app_name);
        else if (portal && (w[1].pending & RX_PEER_CLOSED)) {
            rx_handle_close(portal);
            portal = 0;
        }
    }
    if (portal) rx_handle_close(portal);
    int64_t code = rx_wait_process(proc);
    rx_handle_close(job);
    printf("appctl: %s ended (%lld)\n", ID, (long long)code);
    return (int)code;
}

static int usage(void) {
    fprintf(stderr, "usage: appctl run ID [ARG...] | open ID FILE... | view ID [FILE...] | permissions ID\n");
    return 2;
}

int main(int argc, char **argv) {
    if (argc < 3) return usage();
    ID = argv[2];
    if (!valid_id(ID)) {
        fprintf(stderr, "appctl: %s: not an app ID\n", ID);
        return 2;
    }
    struct passwd *pw = getpwuid(getuid());
    if (!pw || !pw->pw_dir || pw->pw_dir[0] != '/') {
        fprintf(stderr, "appctl: who are you? (not in /etc/passwd)\n");
        return 1;
    }
    if (load_manifest() < 0) return 1;
    permissions(pw->pw_dir);
    build_view(pw->pw_dir);
    if (!strcmp(argv[1], "run")) return run(argc - 3, argv + 3, pw);
    // open ID FILE... (and view ID FILE...): those files in its view too
    static char files[24][600];
    static char *fargs[24];
    int nf = 0;
    if (!strcmp(argv[1], "open") || !strcmp(argv[1], "view")) {
        if (argc - 3 > 24) {
            fprintf(stderr, "appctl: at most 24 files\n");
            return 2;
        }
        for (int k = 3; k < argc; k++) {
            if (add_file(argv[k], files[nf], sizeof files[nf]) < 0) return 1;
            fargs[nf] = files[nf];
            nf++;
        }
    }
    if (!strcmp(argv[1], "open")) {
        if (!nf) return usage();
        for (int k = 0; k < nf; k++) printf("appctl: %s gets %s (%s)\n", ID, fargs[k], mode_name(vmode[vn - nf + k]));
        fflush(stdout);
        return run(nf, fargs, pw);
    }
    if (!strcmp(argv[1], "view")) {
        for (int k = 0; k < vn; k++) printf("%s %s\n", mode_name(vmode[k]), vpath[k]);
        return 0;
    }
    if (!strcmp(argv[1], "permissions")) {
        printf("asks for:");
        for (int k = 0; KNOWN[k]; k++)
            if (has_word(asked, KNOWN[k])) printf(" %s", KNOWN[k]);
        printf("\ngets:");
        for (int k = 0; KNOWN[k]; k++)
            if (granted[k]) printf(" %s", KNOWN[k]);
        printf("\n%s\n", user_set ? "(narrowed in ~/.config/apps/permissions)" : "(all it asks for)");
        return 0;
    }
    return usage();
}
