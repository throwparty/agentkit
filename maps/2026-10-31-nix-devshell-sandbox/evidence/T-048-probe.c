/* T-048: can readiness detect Seatbelt without confining the caller? */
#include <dlfcn.h>
#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

#include <sys/wait.h>

extern int sandbox_init(const char *profile, unsigned long long flags,
                        char **errorbuf);

static double now_ms(void) {
    struct timespec t;
    clock_gettime(CLOCK_MONOTONIC, &t);
    return (double)t.tv_sec * 1000.0 + (double)t.tv_nsec / 1e6;
}

static int apply(const char *profile) {
    char *error = NULL;
    if (sandbox_init(profile, 0, &error) != 0) {
        fprintf(stderr, "sandbox_init: %s\n", error ? error : "unknown error");
        return 70;
    }
    return 0;
}

int main(int argc, char **argv) {
    if (argc < 2) { fprintf(stderr, "usage: %s symbol|apply|invalid|time [n]\n", argv[0]); return 64; }

    if (!strcmp(argv[1], "symbol")) {
        void *direct = dlsym(RTLD_DEFAULT, "sandbox_init");
        void *handle = dlopen("/usr/lib/libSystem.B.dylib", RTLD_NOW);
        void *system = handle ? dlsym(handle, "sandbox_init") : NULL;
        printf("dlsym(RTLD_DEFAULT,\"sandbox_init\")=%p\n", direct);
        printf("dlopen(libSystem)=%p dlsym(handle,\"sandbox_init\")=%p\n", handle, system);
        if (!direct) { printf("symbol missing: %s\n", dlerror()); return 1; }
        return 0;
    }

    if (!strcmp(argv[1], "apply")) {
        if (apply("(version 1)\n(deny default)\n")) return 70;
        printf("applied\n");
        _exit(0);
    }

    if (!strcmp(argv[1], "invalid")) {
        if (apply("(version 1)\n(deny nonsense)\n")) return 70;
        printf("unexpectedly applied\n");
        _exit(0);
    }

    if (!strcmp(argv[1], "time")) {
        int runs = argc > 2 ? atoi(argv[2]) : 30;
        double samples[256];
        for (int i = 0; i < runs && i < 256; i++) {
            double start = now_ms();
            pid_t pid = fork();
            if (pid == 0) {
                if (apply("(version 1)\n(deny default)\n")) _exit(70);
                _exit(0);
            }
            int status = 0;
            waitpid(pid, &status, 0);
            double end = now_ms();
            if (!WIFEXITED(status) || WEXITSTATUS(status) != 0) {
                fprintf(stderr, "probe failed: status=0x%x\n", (unsigned)status);
                return 1;
            }
            samples[i] = end - start;
        }
        for (int i = 1; i < runs; i++)
            for (int j = i; j > 0 && samples[j - 1] > samples[j]; j--) {
                double swap = samples[j]; samples[j] = samples[j - 1]; samples[j - 1] = swap;
            }
        double median = samples[runs / 2];
        double p95 = samples[(95 * runs + 99) / 100 - 1];
        printf("runs=%d median_ms=%.3f p95_ms=%.3f min_ms=%.3f max_ms=%.3f\n",
               runs, median, p95, samples[0], samples[runs - 1]);
        return 0;
    }

    fprintf(stderr, "unknown mode\n");
    return 64;
}
