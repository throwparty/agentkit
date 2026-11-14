/* T-045: a child that leaves the process group and stays confined. */
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

int main(int argc, char **argv) {
    if (argc < 4) return 64;
    if (setsid() < 0) perror("setsid");
    FILE *pid_file = fopen(argv[3], "w");
    if (pid_file) { fprintf(pid_file, "%ld\n", (long)getpid()); fclose(pid_file); }
    int reported = 0;
    for (;;) {
        FILE *f = fopen(argv[1], "a");
        if (f) { fputs("x\n", f); fclose(f); }
        int fd = open(argv[2], O_WRONLY | O_CREAT | O_TRUNC, 0644);
        if (fd >= 0) { write(fd, "escaped\n", 8); close(fd); }
        else if (!reported) {
            fprintf(stderr, "outside write: %s\n", strerror(errno));
            reported = 1;
        }
        sleep(1);
    }
}
