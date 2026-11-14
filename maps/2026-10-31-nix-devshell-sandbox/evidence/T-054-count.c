#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

extern int sandbox_init_with_parameters(const char *profile,
                                        unsigned long long flags,
                                        const char *const parameters[],
                                        char **errorbuf);

int main(int argc, char **argv) {
    if (argc < 3) {
        fprintf(stderr, "usage: %s <count> <path>\n", argv[0]);
        return 64;
    }
    int count = atoi(argv[1]);
    const char *fixed = argc > 3 ? argv[2] : NULL;

    if (count < 0 || count > 1000000) {
        fprintf(stderr, "count must be between 0 and 1000000\n");
        return 64;
    }

    size_t size = (size_t)count * 64 + 4096;
    char *profile = malloc(size);
    if (!profile) {
        perror("malloc");
        return 70;
    }
    size_t used = 0;
#define APPEND(...) do { \
    size_t remaining = size - used; \
    int written = snprintf(profile + used, remaining, __VA_ARGS__); \
    if (written < 0 || (size_t)written >= remaining) { \
        fprintf(stderr, "profile buffer too small\n"); \
        return 70; \
    } \
    used += (size_t)written; \
} while (0)
    APPEND("(version 1)\n(deny default)\n(allow process-exec)\n(allow file-read*\n"
           "  (literal \"/\")\n  (subpath \"/usr/bin\")\n  (subpath \"/bin\")\n"
           "  (subpath \"/usr/lib\")\n  (subpath \"/System\")\n  (subpath \"/Library\")\n");
    for (int i = 0; i < count; i++)
        APPEND("  (subpath (param \"P%d\"))\n", i);
    APPEND(")\n(allow file-read-metadata)\n(allow sysctl-read)\n(allow mach-lookup)\n");
#undef APPEND

    const char **params = calloc((size_t)count * 2 + 1, sizeof(char *));
    char **names = calloc((size_t)count, sizeof(char *));
    char **values = calloc((size_t)count, sizeof(char *));
    for (int i = 0; i < count; i++) {
        names[i] = malloc(16);
        snprintf(names[i], 16, "P%d", i);
        values[i] = malloc(80);
        snprintf(values[i], 80, "/nix/store/%032d-pkg-%d", i, i);
        params[i * 2] = names[i];
        params[i * 2 + 1] = fixed ? fixed : values[i];
    }
    params[count * 2] = NULL;

    char *error = NULL;
    if (sandbox_init_with_parameters(profile, 0, params, &error) != 0) {
        fprintf(stderr, "count=%d profile_bytes=%zu failed: %s\n", count, used,
                error ? error : "unknown");
        return 70;
    }
    printf("count=%d profile_bytes=%zu applied\n", count, used);
    fflush(stdout);
    execl("/usr/bin/true", "/usr/bin/true", (char *)NULL);
    perror("execl");
    return 126;
}
