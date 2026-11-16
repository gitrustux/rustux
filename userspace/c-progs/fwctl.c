// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// fwctl: the firewall in netd (network step D5, docs/NETWORK.md, man
// fwctl, man firewall.conf).
//
//   fwctl [status]                                  the rules, with hits
//   fwctl allow|deny [in|out] PROTO [PORT[-PORT]] [from|to ADDRESS[/PREFIX]]
//                                                   a rule, first (it wins)
//   fwctl append allow|deny ...                     a rule, last
//   fwctl delete N | fwctl delete allow|deny ...    remove a rule
//   fwctl default in|out allow|deny                 the policy when no rule matches
//   fwctl enable | disable
//   fwctl reload                                    /etc/firewall.conf again
//   fwctl save                                      the rules into /etc/firewall.conf
//
// PROTO is tcp, udp, icmp or any; "in" when neither in nor out is given.
// Rules are about new conversations; answers to what this computer sent
// always pass. Changes are root's and last until netd restarts, unless
// saved.

#include "netclient.h"
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static const char *prog = "fwctl";

static void usage(void) {
    fprintf(stderr,
            "usage: fwctl [status]\n"
            "       fwctl allow|deny [in|out] tcp|udp|icmp|any [PORT[-PORT]] [from|to ADDRESS[/PREFIX]]\n"
            "       fwctl append allow|deny ...\n"
            "       fwctl delete N | delete allow|deny ...\n"
            "       fwctl default in|out allow|deny\n"
            "       fwctl enable | disable | reload | save\n");
    exit(1);
}

static void join(char *out, size_t cap, const char *head, int argc, char **argv) {
    snprintf(out, cap, "%s", head);
    for (int i = 0; i < argc; i++) {
        size_t n = strlen(out);
        snprintf(out + n, cap - n, " %s", argv[i]);
    }
}

static void status(void) {
    char *t = net_request(prog, "fw list");
    // enabled yes|no / default in X dropped N / default out X dropped N /
    // rule K TEXT hits N
    char *save = 0;
    for (char *line = strtok_r(t, "\n", &save); line; line = strtok_r(0, "\n", &save)) {
        if (!strcmp(line, "ok")) break;
        if (!strncmp(line, "enabled ", 8)) {
            printf("Firewall: %s\n", !strcmp(line + 8, "yes") ? "on" : "off (everything passes)");
        } else if (!strncmp(line, "default ", 8)) {
            char dir[8], act[8];
            unsigned long dropped = 0;
            const char *p = line + 8;
            size_t k = strcspn(p, " ");
            snprintf(dir, sizeof dir, "%.*s", (int)k, p);
            p += k + 1;
            k = strcspn(p, " ");
            snprintf(act, sizeof act, "%.*s", (int)k, p);
            const char *d = strstr(p, "dropped ");
            if (d) dropped = strtoul(d + 8, 0, 10);
            printf("New connections %s: %s by default (%lu packets dropped)\n", !strcmp(dir, "in") ? "in" : "out",
                   !strcmp(act, "allow") ? "allowed" : "denied", dropped);
        } else if (!strncmp(line, "rule ", 5)) {
            char *hits = strstr(line, " hits ");
            if (hits) *hits = 0;
            char *sp = strchr(line + 5, ' ');
            if (!sp) continue;
            *sp = 0;
            printf("%3s  %-50s %s hits\n", line + 5, sp + 1, hits ? hits + 6 : "0");
        }
    }
}

int main(int argc, char **argv) {
    char req[512];
    if (argc == 1 || !strcmp(argv[1], "status") || !strcmp(argv[1], "list")) {
        status();
        return 0;
    }
    const char *cmd = argv[1];
    if (!strcmp(cmd, "-h") || !strcmp(cmd, "--help")) usage();
    if (!strcmp(cmd, "allow") || !strcmp(cmd, "deny")) {
        join(req, sizeof req, "fw add 1", argc - 1, argv + 1);
    } else if (!strcmp(cmd, "append") && argc > 2) {
        join(req, sizeof req, "fw add", argc - 2, argv + 2);
    } else if (!strcmp(cmd, "delete") && argc == 3) {
        snprintf(req, sizeof req, "fw del %s", argv[2]);
    } else if (!strcmp(cmd, "delete") && argc > 3) {
        join(req, sizeof req, "fw delrule", argc - 2, argv + 2);
    } else if (!strcmp(cmd, "default") && argc == 4) {
        snprintf(req, sizeof req, "fw default %s %s", argv[2], argv[3]);
    } else if (argc == 2 && (!strcmp(cmd, "enable") || !strcmp(cmd, "disable") || !strcmp(cmd, "reload") || !strcmp(cmd, "save"))) {
        snprintf(req, sizeof req, "fw %s", cmd);
    } else {
        usage();
    }
    net_request(prog, req);
    return 0;
}
