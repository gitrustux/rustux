/*
 * Downloads for the framebuffer frontend on Rustica (written for Rustica,
 * 2026-11-26; GPL 2, as NetSurf). Upstream's framebuffer frontend has no
 * download support; here a download goes to ~/Downloads (NAME, or
 * "NAME (2)" if that is taken), written as NAME.part and renamed when it
 * is complete. Its progress is the window's status line; a notification
 * says when it finished or failed.
 */

#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

#ifdef __rustux__
#include <rustux/window.h>
#endif

#include "utils/nsurl.h"
#include "netsurf/download.h"
#include "desktop/download.h"

#include <libnsfb.h>
#include <libnsfb_event.h>

#include "framebuffer/gui.h"
#include "framebuffer/fbtk.h"
#include "framebuffer/download.h"

struct gui_download_window {
	struct gui_window *parent;
	FILE *f;
	char path[1024];	/* the file when complete */
	char part[1040];	/* while it is written */
	const char *name;	/* within path */
	unsigned long long total, done;
	unsigned int shown;	/* the last percentage (or MiB) in the status */
};

static void say(struct gui_download_window *dw, const char *text)
{
	if (dw->parent != NULL && dw->parent->status != NULL)
		fbtk_set_text(dw->parent->status, text);
}

static void notify(const char *text)
{
#ifdef __rustux__
	rx_window_notify(0, text);
#else
	(void)text;
#endif
}

static void human(unsigned long long n, char *out, size_t size)
{
	if (n >= 1024ull * 1024 * 1024)
		snprintf(out, size, "%.1f GB", n / (1024.0 * 1024 * 1024));
	else if (n >= 1024 * 1024)
		snprintf(out, size, "%.1f MB", n / (1024.0 * 1024));
	else if (n >= 1024)
		snprintf(out, size, "%.0f KB", n / 1024.0);
	else
		snprintf(out, size, "%llu bytes", n);
}

static struct gui_download_window *
fb_download_create(download_context *ctx, struct gui_window *parent)
{
	struct gui_download_window *dw = calloc(1, sizeof *dw);
	const char *home = getenv("HOME");
	const char *name = download_context_get_filename(ctx);
	char dir[512], base[256], ext[64];
	struct stat st;

	if (dw == NULL)
		return NULL;
	dw->parent = parent;
	dw->total = download_context_get_total_length(ctx);

	/* ~/Downloads, made if missing */
	snprintf(dir, sizeof dir, "%s/Downloads", home && *home ? home : "/tmp");
	mkdir(dir, 0755);

	/* the name without slashes; "download" if there is none */
	if (name == NULL || *name == '\0')
		name = "download";
	snprintf(base, sizeof base, "%s", name);
	for (char *c = base; *c; c++)
		if (*c == '/')
			*c = '_';

	/* a free name: NAME, then NAME (2).EXT, NAME (3).EXT, ... */
	ext[0] = '\0';
	char *dot = strrchr(base, '.');
	if (dot != NULL && dot != base) {
		snprintf(ext, sizeof ext, "%s", dot);
		*dot = '\0';
	}
	snprintf(dw->path, sizeof dw->path, "%s/%s%s", dir, base, ext);
	for (int n = 2; stat(dw->path, &st) == 0 && n < 1000; n++)
		snprintf(dw->path, sizeof dw->path, "%s/%s (%d)%s", dir, base, n, ext);
	dw->name = dw->path + strlen(dir) + 1;
	snprintf(dw->part, sizeof dw->part, "%s.part", dw->path);

	dw->f = fopen(dw->part, "wb");
	if (dw->f == NULL) {
		char msg[1200];
		snprintf(msg, sizeof msg, "Cannot save %s: %s", dw->path, strerror(errno));
		say(dw, msg);
		notify(msg);
		free(dw);
		return NULL;
	}

	char msg[1200], size[32] = "";
	if (dw->total)
		human(dw->total, size, sizeof size);
	snprintf(msg, sizeof msg, "Downloading %s%s%s%s to Downloads", dw->name,
		 dw->total ? " (" : "", size, dw->total ? ")" : "");
	say(dw, msg);
	return dw;
}

static nserror
fb_download_data(struct gui_download_window *dw, const char *data, unsigned int size)
{
	if (fwrite(data, 1, size, dw->f) != size)
		return NSERROR_SAVE_FAILED;
	dw->done += size;

	/* the status: each new percent, or each MB when the size is unknown */
	unsigned int now = dw->total ? (unsigned int)(dw->done * 100 / dw->total)
				     : (unsigned int)(dw->done >> 20);
	if (now != dw->shown) {
		char msg[1200], a[32], b[32];
		dw->shown = now;
		human(dw->done, a, sizeof a);
		if (dw->total) {
			human(dw->total, b, sizeof b);
			snprintf(msg, sizeof msg, "Downloading %s: %u%% (%s of %s)", dw->name, now, a, b);
		} else {
			snprintf(msg, sizeof msg, "Downloading %s: %s", dw->name, a);
		}
		say(dw, msg);
	}
	return NSERROR_OK;
}

static void fb_download_error(struct gui_download_window *dw, const char *error_msg)
{
	char msg[1200];
	fclose(dw->f);
	unlink(dw->part);
	snprintf(msg, sizeof msg, "Download of %s failed: %s", dw->name, error_msg ? error_msg : "error");
	say(dw, msg);
	notify(msg);
	free(dw);
}

static void fb_download_done(struct gui_download_window *dw)
{
	char msg[1200], a[32];
	int ok = fclose(dw->f) == 0 && rename(dw->part, dw->path) == 0;
	human(dw->done, a, sizeof a);
	if (ok)
		snprintf(msg, sizeof msg, "Downloaded %s (%s) to Downloads", dw->name, a);
	else
		snprintf(msg, sizeof msg, "Could not save %s: %s", dw->path, strerror(errno));
	say(dw, msg);
	notify(msg);
	free(dw);
}

static struct gui_download_table download_table = {
	.create = fb_download_create,
	.data = fb_download_data,
	.error = fb_download_error,
	.done = fb_download_done,
};

struct gui_download_table *framebuffer_download_table = &download_table;
