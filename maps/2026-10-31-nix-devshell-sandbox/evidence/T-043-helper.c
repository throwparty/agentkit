/* Seatbelt probe helper: renders a deny-default profile, applies it, execs. */
#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

extern int sandbox_init(const char *profile, unsigned long long flags,
                        char **errorbuf);

static const char *BASELINE[] = {
    "/usr/bin", "/usr/lib", "/usr/share", "/usr/libexec",
    "/System/Library", "/System/Applications",
    "/Library/Developer", "/Applications/Xcode.app/Contents/Developer",
    "/Library/Frameworks", "/bin", "/sbin", "/private/etc",
    "/private/var/db/dyld", "/private/var/select", "/dev",
};

static const char *MACH[] = {
    "com.apple.system.logger",
    "com.apple.system.notification_center",
    "com.apple.distributed_notifications@Uv3",
    "com.apple.logd",
    "com.apple.lsd.mapdb",
    "com.apple.fonts",
    "com.apple.FontObjectsServer",
    "com.apple.PowerManagement.control",
    "com.apple.system.opendirectoryd.libinfo",
    "com.apple.system.opendirectoryd.membership",
    "com.apple.bsd.dirhelper",
    "com.apple.SystemConfiguration.configd",
};

static char *must_realpath(const char *path) {
    char *resolved = realpath(path, NULL);
    if (!resolved) {
        fprintf(stderr, "realpath %s: %s\n", path, strerror(errno));
        exit(70);
    }
    return resolved;
}

int main(int argc, char **argv) {
    char *workspace = NULL;
    char **reads = calloc((size_t)argc, sizeof(char *));
    char **writes = calloc((size_t)argc, sizeof(char *));
    int nreads = 0, nwrites = 0, i = 1;
    const char *dump = NULL;

    int developer = 1;
    for (; i < argc; i++) {
        if (strcmp(argv[i], "--") == 0) { i++; break; }
        if (strcmp(argv[i], "--without-developer") == 0) developer = 0;
        else if (strcmp(argv[i], "--workspace") == 0 && i + 1 < argc)
            workspace = must_realpath(argv[++i]);
        else if (strcmp(argv[i], "--read") == 0 && i + 1 < argc)
            reads[nreads++] = must_realpath(argv[++i]);
        else if (strcmp(argv[i], "--write") == 0 && i + 1 < argc)
            writes[nwrites++] = must_realpath(argv[++i]);
        else if (strcmp(argv[i], "--dump-profile") == 0 && i + 1 < argc)
            dump = argv[++i];
        else {
            fprintf(stderr, "unknown argument: %s\n", argv[i]);
            return 64;
        }
    }
    if (!workspace || i >= argc) {
        fprintf(stderr, "usage: %s --workspace <p> [--read <p>] [--write <p>] "
                        "[--dump-profile <p>] -- <cmd...>\n", argv[0]);
        return 64;
    }

    char *profile = NULL;
    size_t length = 0;
    FILE *out = open_memstream(&profile, &length);
    if (!out) { perror("open_memstream"); return 70; }

    fputs("(version 1)\n(deny default (with message \"litterbox-probe\"))\n"
          "(allow process-exec)\n(allow process-fork)\n"
          "(allow process-info* (target same-sandbox))\n"
          "(allow signal (target same-sandbox))\n"
          "(allow file-read*\n  (literal \"/\")\n", out);
    for (size_t b = 0; b < sizeof(BASELINE) / sizeof(BASELINE[0]); b++) {
        if (!developer && (strcmp(BASELINE[b], "/Library/Developer") == 0 ||
                           strcmp(BASELINE[b], "/Applications/Xcode.app/Contents/Developer") == 0))
            continue;
        fprintf(out, "  (subpath \"%s\")\n", BASELINE[b]);
    }
    fprintf(out, "  (subpath \"%s\")\n", workspace);
    for (int r = 0; r < nreads; r++) fprintf(out, "  (subpath \"%s\")\n", reads[r]);
    fputs(")\n(allow file-read-metadata)\n", out);
    fprintf(out, "(allow file-write*\n  (literal \"/dev/null\")\n"
                 "  (subpath \"%s\")\n", workspace);
    for (int w = 0; w < nwrites; w++) fprintf(out, "  (subpath \"%s\")\n", writes[w]);
    fputs(")\n(allow sysctl-read)\n(allow mach-lookup\n", out);
    for (size_t m = 0; m < sizeof(MACH) / sizeof(MACH[0]); m++)
        fprintf(out, "  (global-name \"%s\")\n", MACH[m]);
    fputs(")\n", out);
    fclose(out);

    if (dump) {
        FILE *d = fopen(dump, "w");
        if (!d) { perror(dump); return 70; }
        fwrite(profile, 1, length, d);
        fclose(d);
    }

    char *error = NULL;
    if (sandbox_init(profile, 0, &error) != 0) {
        fprintf(stderr, "sandbox_init: %s\n", error ? error : "unknown error");
        return 70;
    }
    execvp(argv[i], &argv[i]);
    fprintf(stderr, "exec %s: %s\n", argv[i], strerror(errno));
    return 126;
}
