// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// Starting programs: the ELF loader. The program's file is read through
// the filesystem service (or, in the boot image, mapped from the image's
// VMO: code is then shared, not copied), its segments are mapped
// into a new process with their own permissions, a stack is mapped at the
// top of its address space, and its first thread starts at the entry point
// with a bootstrap channel holding the processargs message (handles,
// arguments, environment), as the kernel does for init. A file starting
// with #! starts its interpreter instead (with the file's path as an
// argument).

#include "libc.h"
#include <errno.h>
#include <fcntl.h>
#include <rustux/fs.h>
#include <rustux/processargs.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

#define MAX_SPAWN_HANDLES 24
#define STACK_SIZE (256 * 1024)
#define ROOT_END 0x7ffffffff000ull
#define MAX_IMAGE (64ull << 20)

static uint64_t rd(const unsigned char *p, int n) {
    uint64_t v = 0;
    for (int i = n - 1; i >= 0; i--) v = (v << 8) | p[i];
    return v;
}

// Load `elf` into the process behind `vmar`; returns the entry point or 0.
// With `src` (a VMO holding the file at `src_off`, page-aligned), segments
// that are not writable are mapped from it directly instead of copied:
// programs in the boot image start without copying their code.
static uint64_t load_elf(rx_handle_t vmar, const unsigned char *elf, size_t len, rx_handle_t src, uint64_t src_off) {
    if (len < 64 || memcmp(elf, "\177ELF", 4) || elf[4] != 2 || rd(elf + 18, 2) != 0x3E) return 0;
    uint64_t entry = rd(elf + 24, 8), phoff = rd(elf + 32, 8);
    uint64_t phentsize = rd(elf + 54, 2), phnum = rd(elf + 56, 2);
    if (phoff + phnum * phentsize > len || phentsize < 56) return 0;
    if (src_off & 0xFFF) src = 0;

    uint64_t lo = ~0ull, hi = 0;
    for (uint64_t i = 0; i < phnum; i++) {
        const unsigned char *ph = elf + phoff + i * phentsize;
        if (rd(ph, 4) != 1 || !rd(ph + 40, 8)) continue; // PT_LOAD with memory
        uint64_t va = rd(ph + 16, 8), off = rd(ph + 8, 8), fsz = rd(ph + 32, 8), msz = rd(ph + 40, 8);
        if (fsz > msz || off + fsz > len) return 0;
        if ((va & 0xFFF) != (off & 0xFFF)) src = 0; // not mappable from the file
        if ((va & ~0xFFFull) < lo) lo = va & ~0xFFFull;
        if (((va + msz + 0xFFF) & ~0xFFFull) > hi) hi = (va + msz + 0xFFF) & ~0xFFFull;
    }
    if (lo == ~0ull || lo < RX_ROOT_VMAR_BASE || hi > ROOT_END - STACK_SIZE || hi - lo > MAX_IMAGE) return 0;

    // Per page: permissions, whether a writable segment touches it, and
    // (direct pages) the file offset it maps (+1, 0 = copied)
    uint64_t pages = (hi - lo) / 4096;
    unsigned char *perm = calloc(pages, 1), *wr = calloc(pages, 1);
    uint64_t *foff = calloc(pages, sizeof *foff);
    uint64_t ok = perm && wr && foff;
    for (uint64_t i = 0; ok && i < phnum; i++) {
        const unsigned char *ph = elf + phoff + i * phentsize;
        if (rd(ph, 4) != 1 || !rd(ph + 40, 8)) continue;
        uint64_t flags = rd(ph + 4, 4), va = rd(ph + 16, 8), msz = rd(ph + 40, 8);
        unsigned p = RX_VM_PERM_READ;
        if (flags & 2) p |= RX_VM_PERM_WRITE;
        if (flags & 1) p |= RX_VM_PERM_EXECUTE;
        uint64_t first = ((va & ~0xFFFull) - lo) / 4096, last = (((va + msz + 0xFFF) & ~0xFFFull) - lo) / 4096;
        for (uint64_t pg = first; pg < last; pg++) {
            perm[pg] |= (unsigned char)p;
            if (flags & 2) wr[pg] = 1;
        }
    }
    // Which segments map directly: not writable, no page shared with a
    // writable one, and no zero-filled pages beyond the file's bytes
    unsigned char direct_seg[64] = { 0 };
    for (uint64_t i = 0; ok && src && i < phnum && i < 64; i++) {
        const unsigned char *ph = elf + phoff + i * phentsize;
        if (rd(ph, 4) != 1 || !rd(ph + 40, 8)) continue;
        uint64_t flags = rd(ph + 4, 4), va = rd(ph + 16, 8), off = rd(ph + 8, 8);
        uint64_t fsz = rd(ph + 32, 8), msz = rd(ph + 40, 8);
        if (flags & 2) continue;
        if (((va + msz + 0xFFF) & ~0xFFFull) > ((va + fsz + 0xFFF) & ~0xFFFull)) continue;
        uint64_t first = ((va & ~0xFFFull) - lo) / 4096, last = (((va + msz + 0xFFF) & ~0xFFFull) - lo) / 4096;
        int shared = 0;
        for (uint64_t pg = first; pg < last; pg++) shared |= wr[pg];
        if (shared) continue;
        direct_seg[i] = 1;
        for (uint64_t pg = first; pg < last; pg++) foff[pg] = src_off + (off & ~0xFFFull) + (pg - first) * 4096 + 1;
    }
    // Everything else is copied into one VMO covering those pages
    uint64_t clo = ~0ull, chi = 0;
    for (uint64_t pg = 0; ok && pg < pages; pg++) {
        if (perm[pg] && !foff[pg]) {
            if (lo + pg * 4096 < clo) clo = lo + pg * 4096;
            chi = lo + (pg + 1) * 4096;
        }
    }
    rx_handle_t copy = 0, src_exec = 0;
    if (ok && chi > clo) {
        rx_handle_t vmo;
        if (rx_vmo_create(chi - clo, 0, &vmo) != RX_OK) ok = 0;
        for (uint64_t i = 0; ok && i < phnum; i++) {
            const unsigned char *ph = elf + phoff + i * phentsize;
            if (rd(ph, 4) != 1 || !rd(ph + 40, 8) || (i < 64 && direct_seg[i])) continue;
            uint64_t va = rd(ph + 16, 8), off = rd(ph + 8, 8), fsz = rd(ph + 32, 8);
            if (fsz) rx_vmo_write(vmo, elf + off, va - clo, fsz);
        }
        if (ok && rx_vmo_replace_as_executable(vmo, 0, &copy) != RX_OK) ok = 0;
    }
    if (ok && src) {
        rx_handle_t dup;
        if (rx_handle_duplicate(src, RX_RIGHT_SAME_RIGHTS, &dup) != RX_OK ||
            rx_vmo_replace_as_executable(dup, 0, &src_exec) != RX_OK) ok = 0;
    }
    // Map runs of pages with the same permissions and source
    for (uint64_t start = 0; start < pages && ok;) {
        uint64_t end = start + 1;
        while (end < pages && perm[end] == perm[start] && !foff[end] == !foff[start] &&
               (!foff[start] || foff[end] == foff[start] + (end - start) * 4096)) end++;
        if (perm[start]) {
            uint64_t addr, va = lo + start * 4096;
            rx_handle_t from = foff[start] ? src_exec : copy;
            uint64_t from_off = foff[start] ? foff[start] - 1 : va - clo;
            if (rx_vmar_map(vmar, perm[start] | RX_VM_SPECIFIC, va - RX_ROOT_VMAR_BASE, from, from_off,
                            (end - start) * 4096, &addr) != RX_OK) ok = 0;
        }
        start = end;
    }
    if (copy) rx_handle_close(copy);
    if (src_exec) rx_handle_close(src_exec);
    free(perm);
    free(wr);
    free(foff);
    return ok ? entry : 0;
}

// Copy strings into `p`, NUL-terminated; returns the bytes used
static size_t put_strings(char *p, char *const v[], uint32_t *count) {
    size_t n = 0;
    uint32_t c = 0;
    for (; v && v[c]; c++) {
        size_t k = strlen(v[c]) + 1;
        if (p) memcpy(p + n, v[c], k);
        n += k;
    }
    *count = c;
    return n;
}

static rx_status_t spawn_image(rx_handle_t job, const char *name, const void *elf, size_t len,
                               rx_handle_t src, uint64_t src_off, char *const argv[], char *const envp[],
                               uint32_t flags, const rx_spawn_handle_t *extra, uint32_t nextra,
                               rx_handle_t *proc_out);

rx_status_t rx_spawn_image(rx_handle_t job, const char *name, const void *elf, size_t len,
                           char *const argv[], char *const envp[], uint32_t flags,
                           const rx_spawn_handle_t *extra, uint32_t nextra, rx_handle_t *proc_out) {
    return spawn_image(job, name, elf, len, 0, 0, argv, envp, flags, extra, nextra, proc_out);
}

static rx_status_t spawn_image(rx_handle_t job, const char *name, const void *elf, size_t len,
                               rx_handle_t src, uint64_t src_off, char *const argv[], char *const envp[],
                               uint32_t flags, const rx_spawn_handle_t *extra, uint32_t nextra,
                               rx_handle_t *proc_out) {
    rx_handle_t hs[MAX_SPAWN_HANDLES];
    uint32_t info[MAX_SPAWN_HANDLES];
    uint32_t n = 0;
    rx_handle_t proc = 0, vmar = 0, thread = 0, boot0 = 0, boot1 = 0;
    rx_status_t r = RX_OK;
    char *msg = 0;

    // The caller's extra handles are ours to close from here on
    for (uint32_t i = 0; i < nextra; i++) {
        if (n + 8 < MAX_SPAWN_HANDLES) {
            hs[n] = extra[i].handle;
            info[n++] = extra[i].info;
        } else {
            rx_handle_close(extra[i].handle);
            r = RX_ERR_OUT_OF_RANGE;
        }
    }
    if (r != RX_OK) goto fail;

    const char *base = strrchr(name, '/') ? strrchr(name, '/') + 1 : name;
    r = rx_process_create(job, base, strlen(base), 0, &proc, &vmar);
    if (r != RX_OK) goto fail;
    uint64_t entry = load_elf(vmar, elf, len, src, src_off);
    if (!entry) { r = RX_ERR_NOT_SUPPORTED; errno = ENOEXEC; goto fail_errno; }

    // Stack at the top of the address space; RSP as if _start were called
    rx_handle_t stack;
    uint64_t addr;
    if ((r = rx_vmo_create(STACK_SIZE, 0, &stack)) != RX_OK) goto fail;
    r = rx_vmar_map(vmar, RX_VM_PERM_READ | RX_VM_PERM_WRITE | RX_VM_SPECIFIC,
                    ROOT_END - STACK_SIZE - RX_ROOT_VMAR_BASE, stack, 0, STACK_SIZE, &addr);
    rx_handle_close(stack);
    if (r != RX_OK) goto fail;
    if ((r = rx_thread_create(proc, "main", 4, 0, &thread)) != RX_OK) goto fail;

    // Standard handles first, then file descriptors, namespace, extras
    rx_handle_t std[4] = { 0, 0, 0, vmar };
    uint32_t std_info[4] = { RX_PA_PROC_SELF, RX_PA_THREAD_SELF, RX_PA_JOB_DEFAULT, RX_PA_VMAR_ROOT };
    rx_handle_duplicate(proc, RX_RIGHT_SAME_RIGHTS, &std[0]);
    rx_handle_duplicate(thread, RX_RIGHT_SAME_RIGHTS, &std[1]);
    rx_handle_duplicate(job, RX_RIGHT_SAME_RIGHTS, &std[2]);
    vmar = 0;
    rx_handle_t all[MAX_SPAWN_HANDLES];
    uint32_t all_info[MAX_SPAWN_HANDLES], m = 0;
    for (int i = 0; i < 4; i++) { all[m] = std[i]; all_info[m++] = std_info[i]; }
    if (flags & RX_SPAWN_CLONE_STDIO) {
        for (int fd = 0; fd < 3; fd++) {
            struct fd *f = &__fds[fd];
            if (f->kind == RX_FD_NONE || f->kind == RX_FD_LOG) continue;
            rx_handle_t h = __fd_export(fd);
            if (h) { all[m] = h; all_info[m++] = RX_FD_INFO(fd, f->kind); }
        }
    }
    if ((flags & RX_SPAWN_CLONE_NS) && rx_ns_root()) {
        rx_handle_t c, s;
        if (rx_channel_create(0, &c, &s) == RX_OK) {
            if (__fs_call(rx_ns_root(), RX_FS_OP_CLONE, 0, 0, 0, 0, s, 0, 0, 0) >= 0) {
                all[m] = c;
                all_info[m++] = RX_PA_HND(RX_PA_NS_DIR, 0);
            } else {
                rx_handle_close(c);
            }
        }
    }
    for (uint32_t i = 0; i < n; i++) { all[m] = hs[i]; all_info[m++] = info[i]; }
    n = 0; // all[] owns them now

    // The processargs message
    char *const default_argv[] = { (char *)name, 0 };
    extern char **environ;
    uint32_t argc, envc;
    size_t alen = put_strings(0, argv ? argv : default_argv, &argc);
    size_t elen = put_strings(0, envp ? envp : environ, &envc);
    size_t hdr = sizeof(rx_proc_args_t) + 4ull * m;
    if (alen + elen > 32768) { r = RX_ERR_OUT_OF_RANGE; errno = E2BIG; goto fail_all; }
    msg = malloc(hdr + alen + elen);
    if (!msg) { r = RX_ERR_NO_MEMORY; goto fail_all; }
    rx_proc_args_t pa = { RX_PROCARGS_PROTOCOL, RX_PROCARGS_VERSION, sizeof pa,
                          (uint32_t)hdr, argc, (uint32_t)(hdr + alen), envc, 0, 0 };
    memcpy(msg, &pa, sizeof pa);
    memcpy(msg + sizeof pa, all_info, 4ull * m);
    put_strings(msg + hdr, argv ? argv : default_argv, &argc);
    put_strings(msg + hdr + alen, envp ? envp : environ, &envc);

    if ((r = rx_channel_create(0, &boot0, &boot1)) != RX_OK) goto fail_all;
    r = rx_channel_write(boot0, 0, msg, (uint32_t)(hdr + alen + elen), all, m); // moves all[]
    m = 0;
    rx_handle_close(boot0);
    free(msg);
    msg = 0;
    if (r != RX_OK) goto fail;
    r = rx_process_start(proc, thread, entry, ROOT_END - 8, boot1, 0); // boot1 moves
    boot1 = 0;
    if (r != RX_OK) goto fail;
    rx_handle_close(thread);
    if (proc_out) *proc_out = proc; else rx_handle_close(proc);
    return RX_OK;

fail_all:
    for (uint32_t i = 0; i < m; i++) rx_handle_close(all[i]);
fail:
    errno = rx_status_to_errno(r);
fail_errno:
    free(msg);
    for (uint32_t i = 0; i < n; i++) rx_handle_close(hs[i]);
    if (boot1) rx_handle_close(boot1);
    if (thread) rx_handle_close(thread);
    if (vmar) rx_handle_close(vmar);
    if (proc) {
        rx_task_kill(proc);
        rx_handle_close(proc);
    }
    return r;
}

rx_status_t rx_spawn_etc(rx_handle_t job, const char *path, char *const argv[], char *const envp[],
                         uint32_t flags, const rx_spawn_handle_t *handles, uint32_t nhandles,
                         rx_handle_t *proc_out) {
    int fd = open(path, O_RDONLY);
    struct stat st;
    void *image = 0;
    rx_status_t r = RX_ERR_NOT_FOUND;
    if (fd < 0) goto fail;
    if (fstat(fd, &st) < 0 || S_ISDIR(st.st_mode)) { r = RX_ERR_ACCESS_DENIED; errno = EACCES; goto fail; }
    if ((uint64_t)st.st_size > MAX_IMAGE) { r = RX_ERR_OUT_OF_RANGE; errno = E2BIG; goto fail; }
    // A script: "#!INTERPRETER [ARG]" starts INTERPRETER [ARG] PATH ARGS...
    {
        char head[256];
        ssize_t k = read(fd, head, sizeof head - 1);
        if (k >= 2 && head[0] == '#' && head[1] == '!') {
            static int depth;
            head[k] = 0;
            head[strcspn(head, "\n")] = 0;
            char *p = head + 2;
            while (*p == ' ' || *p == '\t') p++;
            char *interp = p;
            while (*p && *p != ' ' && *p != '\t') p++;
            char *arg = 0;
            if (*p) {
                *p++ = 0;
                while (*p == ' ' || *p == '\t') p++;
                if (*p) {
                    arg = p;
                    size_t n = strlen(arg);
                    while (n && (arg[n - 1] == ' ' || arg[n - 1] == '\t' || arg[n - 1] == '\r')) arg[--n] = 0;
                }
            }
            close(fd);
            fd = -1;
            if (!*interp || depth > 0) { r = RX_ERR_NOT_SUPPORTED; errno = ENOEXEC; goto fail; }
            int argc = 0;
            while (argv && argv[argc]) argc++;
            char **v = malloc((size_t)(argc + 3) * sizeof *v);
            if (!v) { r = RX_ERR_NO_MEMORY; errno = ENOMEM; goto fail; }
            int n = 0;
            v[n++] = interp;
            if (arg) v[n++] = arg;
            v[n++] = (char *)path;
            for (int i = 1; i < argc; i++) v[n++] = argv[i];
            v[n] = 0;
            depth++;
            r = rx_spawn_etc(job, interp, v, envp, flags, handles, nhandles, proc_out);
            depth--;
            free(v);
            return r;
        }
        lseek(fd, 0, SEEK_SET);
    }
    // A program in the boot image: map it rather than read it
    rx_handle_t src;
    uint64_t src_off, src_size, mapped;
    if (__fs_get_vmo(fd, &src, &src_off, &src_size) == 0) {
        uint64_t page_off = src_off & ~0xFFFull, span = (src_off - page_off + src_size + 0xFFF) & ~0xFFFull;
        if (src_size && rx_vmar_map(rx_vmar_root, RX_VM_PERM_READ, 0, src, page_off, span, &mapped) == RX_OK) {
            close(fd);
            r = spawn_image(job, path, (const void *)(mapped + (src_off - page_off)), src_size, src, src_off, argv, envp,
                            flags, handles, nhandles, proc_out);
            rx_vmar_unmap(rx_vmar_root, mapped, span);
            rx_handle_close(src);
            return r;
        }
        rx_handle_close(src);
    }
    image = malloc(st.st_size ? (size_t)st.st_size : 1);
    if (!image) { r = RX_ERR_NO_MEMORY; errno = ENOMEM; goto fail; }
    size_t got = 0;
    while (got < (size_t)st.st_size) {
        ssize_t k = read(fd, (char *)image + got, (size_t)st.st_size - got);
        if (k <= 0) break;
        got += (size_t)k;
    }
    close(fd);
    fd = -1;
    r = rx_spawn_image(job, path, image, got, argv, envp, flags, handles, nhandles, proc_out);
    free(image);
    return r;
fail:
    {
        int e = errno;
        if (fd >= 0) close(fd);
        free(image);
        for (uint32_t i = 0; i < nhandles; i++) rx_handle_close(handles[i].handle);
        errno = e;
    }
    return r;
}

rx_status_t rx_spawn(const char *path, char *const argv[], char *const envp[], uint32_t flags,
                     const rx_spawn_handle_t *handles, uint32_t nhandles, rx_handle_t *proc_out) {
    return rx_spawn_etc(rx_job_default, path, argv, envp, flags, handles, nhandles, proc_out);
}

int64_t rx_wait_process(rx_handle_t proc) {
    rx_info_process_t info = { 0, 0, 0, 0 };
    rx_object_wait_one(proc, RX_PROCESS_TERMINATED, RX_TIME_INFINITE, 0);
    rx_object_get_info(proc, RX_INFO_PROCESS, &info, sizeof info, 0, 0);
    rx_handle_close(proc);
    return info.return_code;
}
