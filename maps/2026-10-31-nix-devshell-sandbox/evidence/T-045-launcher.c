/* T-045: leave a confined process group behind for a later process to signal. */
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
    "com.apple.system.logger", "com.apple.system.notification_center",
    "com.apple.distributed_notifications@Uv3", "com.apple.logd",
    "com.apple.lsd.mapdb", "com.apple.fonts",
    "com.apple.FontObjectsServer", "com.apple.PowerManagement.control",
    "com.apple.system.opendirectoryd.libinfo",
    "com.apple.system.opendirectoryd.membership",
    "com.apple.bsd.dirhelper", "com.apple.SystemConfiguration.configd",
};

int main(int argc, char **argv) {
    if (argc < 5) {
        fprintf(stderr, "usage: %s <statefile> <workspace> -- <command...>\n", argv[0]);
        return 64;
    }
    const char *state = argv[1];
    char *workspace = realpath(argv[2], NULL);
    if (!workspace) { perror(argv[2]); return 70; }
    if (strcmp(argv[3], "--") != 0 || argc < 5) { fprintf(stderr, "missing --\n"); return 64; }
    char **command = &argv[4];

    char *profile = NULL;
    size_t length = 0;
    FILE *out = open_memstream(&profile, &length);
    fputs("(version 1)\n(deny default (with message \"litterbox-t045\"))\n"
          "(allow process-exec)\n(allow process-fork)\n"
          "(allow process-info* (target same-sandbox))\n"
          "(allow signal (target same-sandbox))\n"
          "(allow file-read*\n  (literal \"/\")\n", out);
    for (size_t i = 0; i < sizeof(BASELINE) / sizeof(BASELINE[0]); i++)
        fprintf(out, "  (subpath \"%s\")\n", BASELINE[i]);
    fprintf(out, "  (subpath \"%s\")\n)\n", workspace);
    fputs("(allow file-read-metadata)\n", out);
    fprintf(out, "(allow file-write*\n  (literal \"/dev/null\")\n  (subpath \"%s\")\n)\n", workspace);
    fputs("(allow sysctl-read)\n(allow mach-lookup\n", out);
    for (size_t i = 0; i < sizeof(MACH) / sizeof(MACH[0]); i++)
        fprintf(out, "  (global-name \"%s\")\n", MACH[i]);
    fputs(")\n", out);
    fclose(out);

    pid_t pid = fork();
    if (pid < 0) { perror("fork"); return 1; }
    if (pid == 0) {
        if (setsid() < 0) { perror("setsid"); _exit(1); }
        char *error = NULL;
        if (sandbox_init(profile, 0, &error) != 0) {
            fprintf(stderr, "sandbox_init: %s\n", error ? error : "unknown");
            _exit(70);
        }
        execv(command[0], command);
        fprintf(stderr, "exec %s: %s\n", command[0], strerror(errno));
        _exit(126);
    }

    FILE *f = fopen(state, "w");
    if (!f) { perror(state); return 1; }
    fprintf(f, "%ld\n", (long)pid);
    fclose(f);
    fprintf(stderr, "launcher: child pid/pgid=%ld\n", (long)pid);
    return 0;
}
