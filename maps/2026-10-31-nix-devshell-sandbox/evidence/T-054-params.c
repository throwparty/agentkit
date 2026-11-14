#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

extern int sandbox_init_with_parameters(const char *profile,
                                        unsigned long long flags,
                                        const char *const parameters[],
                                        char **errorbuf);

int main(int argc, char **argv) {
    if (argc < 4) {
        fprintf(stderr, "usage: %s <workspace> <file> <cmd...>\n", argv[0]);
        return 64;
    }
    const char *params[] = {"WS", argv[1], "FILE", argv[2], NULL};
    const char *profile =
        "(version 1)\n"
        "(deny default)\n"
        "(allow process-exec)\n"
        "(allow process-fork)\n"
        "(allow file-read*\n"
        "  (literal \"/\")\n"
        "  (subpath \"/usr/bin\")\n"
        "  (subpath \"/bin\")\n"
        "  (subpath \"/usr/lib\")\n"
        "  (subpath \"/System\")\n"
        "  (subpath \"/Library\")\n"
        "  (subpath \"/private/etc\")\n"
        "  (subpath (param \"WS\"))\n"
        "  (literal (param \"FILE\")))\n"
        "(allow file-read-metadata)\n"
        "(allow file-write* (literal \"/dev/null\"))\n"
        "(allow sysctl-read)\n"
        "(allow mach-lookup)\n";
    char *error = NULL;
    if (sandbox_init_with_parameters(profile, 0, params, &error) != 0) {
        fprintf(stderr, "sandbox_init_with_parameters: %s\n",
                error ? error : "unknown");
        return 70;
    }
    execv(argv[3], &argv[3]);
    perror("execv");
    return 126;
}
