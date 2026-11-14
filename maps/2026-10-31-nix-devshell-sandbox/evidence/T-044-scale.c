/* T-044: cost of a Seatbelt profile carrying N read grants. */
#include <dirent.h>
#include <errno.h>
#include <limits.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

#include <sys/wait.h>

extern int sandbox_init(const char *profile, unsigned long long flags,
                        char **errorbuf);

static char **paths;
static int path_count;
static char *ungranted;
static int allow_failure;

static double now_ms(void) {
    struct timespec t;
    clock_gettime(CLOCK_MONOTONIC, &t);
    return (double)t.tv_sec * 1000.0 + (double)t.tv_nsec / 1e6;
}

static int by_string(const void *a, const void *b) {
    return strcmp(*(char *const *)a, *(char *const *)b);
}

static int by_double(const void *a, const void *b) {
    double x = *(const double *)a, y = *(const double *)b;
    return x < y ? -1 : x > y;
}

static void collect(int want) {
    DIR *dir = opendir("/nix/store");
    if (!dir) { perror("opendir /nix/store"); exit(1); }
    size_t capacity = 1u << 16, count = 0;
    char **all = malloc(capacity * sizeof *all);
    struct dirent *entry;
    while ((entry = readdir(dir))) {
        if (entry->d_name[0] == '.') continue;
        char path[PATH_MAX];
        snprintf(path, sizeof path, "/nix/store/%s", entry->d_name);
        char *resolved = realpath(path, NULL);
        if (!resolved) continue;
        if (count == capacity) { capacity *= 2; all = realloc(all, capacity * sizeof *all); }
        all[count++] = resolved;
    }
    closedir(dir);
    qsort(all, count, sizeof *all, by_string);
    size_t distinct = 0;
    for (size_t i = 0; i < count; i++)
        if (i == 0 || strcmp(all[i], all[i - 1]) != 0) all[distinct++] = all[i];
    fprintf(stderr, "store_entries=%zu distinct=%zu want=%d\n", count, distinct, want);
    if ((size_t)want > distinct) { fprintf(stderr, "not enough distinct entries\n"); exit(3); }
    ungranted = strdup(all[want]);
    paths = all;
    path_count = want;
}

static char *build(size_t *length) {
    char *profile = NULL;
    FILE *out = open_memstream(&profile, length);
    fputs("(version 1)\n(deny default)\n"
          "(allow process-exec)\n(allow file-map-executable)\n"
          "(allow file-read*\n  (literal \"/\")\n"
          "  (subpath \"/bin\")\n  (subpath \"/usr\")\n"
          "  (subpath \"/System/Library\")\n  (subpath \"/Library\")\n"
          "  (subpath \"/private/etc\")\n  (subpath \"/dev\")\n", out);
    for (int i = 0; i < path_count; i++)
        fprintf(out, "  (subpath \"%s\")\n", paths[i]);
    fputs(")\n(allow file-read-metadata)\n(allow sysctl-read)\n"
          "(allow process-info* (target same-sandbox))\n"
          "(allow signal (target same-sandbox))\n", out);
    fclose(out);
    return profile;
}

static double run_once(const char *profile, int sandboxed, char *const argv[]) {
    double start = now_ms();
    pid_t pid = fork();
    if (pid == 0) {
        if (sandboxed) {
            char *error = NULL;
            if (sandbox_init(profile, 0, &error) != 0) {
                fprintf(stderr, "sandbox_init: %s\n", error ? error : "(none)");
                _exit(2);
            }
        }
        execv(argv[0], argv);
        fprintf(stderr, "execv %s: %s\n", argv[0], strerror(errno));
        _exit(3);
    }
    if (pid < 0) { perror("fork"); exit(1); }
    int status = 0;
    waitpid(pid, &status, 0);
    double end = now_ms();
    if (!WIFEXITED(status) || WEXITSTATUS(status) != 0) {
        fprintf(stderr, "child status=0x%x\n", (unsigned)status);
        if (!allow_failure) exit(4);
        return -1;
    }
    return end - start;
}

int main(int argc, char **argv) {
    int want = 1, runs = 30, sandboxed = 1;
    char *exec = "/usr/bin/true";
    char *av[4]; int ac = 0; av[ac++] = exec;

    for (int i = 1; i < argc; i++) {
        if (!strcmp(argv[i], "--n")) want = atoi(argv[++i]);
        else if (!strcmp(argv[i], "--runs")) runs = atoi(argv[++i]);
        else if (!strcmp(argv[i], "--bare")) sandboxed = 0;
        else if (!strcmp(argv[i], "--exec")) { exec = argv[++i]; ac = 0; av[ac++] = exec; }
        else if (!strcmp(argv[i], "--arg")) av[ac++] = argv[++i];
        else if (!strcmp(argv[i], "--allow-fail")) allow_failure = 1;
        else if (!strcmp(argv[i], "--outside")) { collect(want); puts(ungranted); return 0; }
        else { fprintf(stderr, "bad argument %s\n", argv[i]); return 1; }
    }
    av[ac] = NULL;

    collect(want);
    size_t length = 0;
    char *profile = build(&length);
    printf("n=%d profile_bytes=%zu mode=%s\n", want, sandboxed ? length : 0,
           sandboxed ? "sandboxed" : "bare");

    double samples[512];
    int taken = 0;
    if (runs > 512) runs = 512;
    for (int i = 0; i < runs; i++) {
        double sample = run_once(profile, sandboxed, av);
        if (sample >= 0) samples[taken++] = sample;
    }
    if (taken == 0) { printf("n=%d no successful runs\n", want); return 0; }
    qsort(samples, taken, sizeof *samples, by_double);
    double median = taken % 2 ? samples[taken / 2]
                              : (samples[taken / 2 - 1] + samples[taken / 2]) / 2;
    int p95 = (95 * taken + 99) / 100 - 1;
    if (p95 >= taken) p95 = taken - 1;
    printf("n=%d ok=%d median_ms=%.3f p95_ms=%.3f min_ms=%.3f max_ms=%.3f first_ms=%.3f\n",
           want, taken, median, samples[p95], samples[0], samples[taken - 1],
           samples[0]);
    if (ungranted) free(ungranted);
    return 0;
}
