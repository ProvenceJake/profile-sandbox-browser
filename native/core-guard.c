#include <errno.h>
#include <libproc.h>
#include <signal.h>
#include <stdint.h>
#include <stdlib.h>
#include <unistd.h>

/* Chromium closes on CDP EOF. Give the independently shipped proxy the same lifetime. */
static int same_process(pid_t pid, uint64_t start) {
    struct rusage_info_v0 info;
    return proc_pid_rusage(pid, RUSAGE_INFO_V0, (rusage_info_t *)&info) == 0
        && info.ri_proc_start_abstime == start;
}

int main(int argc, char **argv) {
    if (argc != 3) return 2;
    pid_t pid = (pid_t)strtol(argv[1], NULL, 10);
    uint64_t start = strtoull(argv[2], NULL, 10);
    if (pid <= 1 || start == 0) return 2;
    char discard[32];
    for (;;) {
        ssize_t count = read(STDIN_FILENO, discard, sizeof(discard));
        if (count > 0 || (count < 0 && errno == EINTR)) continue;
        break;
    }
    if (!same_process(pid, start)) return 0;
    kill(pid, SIGTERM);
    for (int i = 0; i < 40; ++i) {
        usleep(25000);
        if (!same_process(pid, start)) return 0;
    }
    if (same_process(pid, start)) kill(pid, SIGKILL);
    return 0;
}
