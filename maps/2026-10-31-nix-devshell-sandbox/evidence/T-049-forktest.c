/* T-049: apply Seatbelt in a forked child of a threaded process. */
#include <errno.h>
#include <fcntl.h>
#include <pthread.h>
#include <signal.h>
#include <spawn.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

#include <sys/wait.h>

extern char **environ;
extern int sandbox_init(const char *profile, unsigned long long flags,
                        char **errorbuf);

static volatile int running = 1;
static pthread_mutex_t lock = PTHREAD_MUTEX_INITIALIZER;
static long counter;

static void *spin(void *arg) {
    (void)arg;
    while (running) {
        pthread_mutex_lock(&lock);
        counter++;
        pthread_mutex_unlock(&lock);
        void *p = malloc(64);
        free(p);
    }
    return NULL;
}

static double now_ms(void) {
    struct timespec t;
    clock_gettime(CLOCK_MONOTONIC, &t);
    return (double)t.tv_sec * 1000.0 + (double)t.tv_nsec / 1e6;
}

static int child_apply(void) {
    char *error = NULL;
    if (sandbox_init("(version 1)\n(deny default)\n", 0, &error) != 0) {
        fprintf(stderr, "sandbox_init: %s\n", error ? error : "unknown");
        return 70;
    }
    return 0;
}

struct result { int ok, failed, hung, crashed; double worst_ms; };

static void one(struct result *r, int thread_fork) {
    double start = now_ms();
    pid_t pid = fork();
    if (pid == 0) {
        int rc = child_apply();
        _exit(rc);
    }
    double deadline = start + 5000;
    int status = 0;
    for (;;) {
        pid_t done = waitpid(pid, &status, WNOHANG);
        if (done == pid) break;
        if (now_ms() > deadline) {
            kill(pid, SIGKILL);
            waitpid(pid, &status, 0);
            r->hung++;
            fprintf(stderr, "hung: child did not finish in 5s\n");
            return;
        }
        usleep(1000);
    }
    double took = now_ms() - start;
    if (took > r->worst_ms) r->worst_ms = took;
    if (WIFSIGNALED(status)) {
        r->crashed++;
        fprintf(stderr, "crashed: signal %d\n", WTERMSIG(status));
    } else if (WIFEXITED(status) && WEXITSTATUS(status) == 0) {
        r->ok++;
    } else {
        r->failed++;
        fprintf(stderr, "failed: status=0x%x\n", (unsigned)status);
    }
    (void)thread_fork;
}

struct thread_arg { struct result *r; int count; };

static void *fork_loop(void *arg) {
    struct thread_arg *a = arg;
    for (int i = 0; i < a->count; i++) one(a->r, 1);
    return NULL;
}

static void spawn_control(struct result *r, int count, char *self) {
    for (int i = 0; i < count; i++) {
        double start = now_ms();
        pid_t pid;
        char *argv[] = { self, "child", NULL };
        if (posix_spawn(&pid, self, NULL, NULL, argv, environ) != 0) { perror("posix_spawn"); exit(1); }
        int status = 0;
        waitpid(pid, &status, 0);
        double took = now_ms() - start;
        if (took > r->worst_ms) r->worst_ms = took;
        if (WIFEXITED(status) && WEXITSTATUS(status) == 0) r->ok++;
        else { r->failed++; fprintf(stderr, "spawn failed: status=0x%x\n", (unsigned)status); }
    }
}

int main(int argc, char **argv) {
    if (argc > 1 && !strcmp(argv[1], "child")) {
        int rc = child_apply();
        _exit(rc);
    }
    if (argc > 1 && !strcmp(argv[1], "denywrite")) {
        if (child_apply()) _exit(70);
        int fd = open("/tmp/t049-write", O_CREAT | O_WRONLY, 0644);
        if (fd >= 0) { close(fd); _exit(3); }
        _exit(errno == EPERM ? 0 : 4);
    }

    int count = argc > 1 ? atoi(argv[1]) : 200;
    pthread_t threads[4];
    for (int i = 0; i < 4; i++) pthread_create(&threads[i], NULL, spin, NULL);
    usleep(100000);

    struct result main_fork = {0}, worker_fork = {0}, spawn = {0};
    printf("threads=4 iterations=%d\n", count);

    for (int i = 0; i < count; i++) one(&main_fork, 0);
    printf("fork_from_main ok=%d failed=%d hung=%d crashed=%d worst_ms=%.1f\n",
           main_fork.ok, main_fork.failed, main_fork.hung, main_fork.crashed, main_fork.worst_ms);

    pthread_t forker;
    struct thread_arg arg = { &worker_fork, count };
    pthread_create(&forker, NULL, fork_loop, &arg);
    pthread_join(forker, NULL);
    printf("fork_from_worker ok=%d failed=%d hung=%d crashed=%d worst_ms=%.1f\n",
           worker_fork.ok, worker_fork.failed, worker_fork.hung, worker_fork.crashed, worker_fork.worst_ms);

    char *self = realpath(argv[0], NULL);
    if (!self) { perror("realpath argv[0]"); return 1; }
    spawn_control(&spawn, count, self);
    printf("spawn_helper ok=%d failed=%d hung=%d crashed=%d worst_ms=%.1f\n",
           spawn.ok, spawn.failed, spawn.hung, spawn.crashed, spawn.worst_ms);

    running = 0;
    for (int i = 0; i < 4; i++) pthread_join(threads[i], NULL);
    printf("counter=%ld\n", counter);
    return 0;
}
