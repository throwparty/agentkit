#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>

extern int sandbox_init_with_parameters(const char *profile,
                                        unsigned long long flags,
                                        const char *const parameters[],
                                        char **errorbuf);

int main(int argc, char **argv) {
    if (argc < 2) {
        fprintf(stderr, "usage: %s <workspace>\n", argv[0]);
        return 64;
    }
    const char *params[] = {
        "TAG", "litterbox-t055 with spaces",
        "WS", argv[1],
        "M0", "com.apple.system.logger",
        "M1", "com.apple.SystemConfiguration.configd",
        "N0", "localhost:8765",
        NULL,
    };
    const char *profile =
        "(version 1)\n"
        "(deny default (with message (param \"TAG\")))\n"
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
        "  (subpath (param \"WS\")))\n"
        "(allow file-read-metadata)\n"
        "(allow file-write* (literal \"/dev/null\"))\n"
        "(allow sysctl-read)\n"
        "(allow mach-lookup\n"
        "  (global-name (param \"M0\"))\n"
        "  (global-name (param \"M1\")))\n"
        "(allow network-outbound\n"
        "  (remote ip (param \"N0\")))\n";
    char *error = NULL;
    if (sandbox_init_with_parameters(profile, 0, params, &error) != 0) {
        fprintf(stderr, "sandbox_init_with_parameters: %s\n",
                error ? error : "unknown");
        return 70;
    }
    printf("applied\n");
    fflush(stdout);
    execl("/usr/bin/true", "/usr/bin/true", (char *)NULL);
    perror("execl");
    return 126;
}
