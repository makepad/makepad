// Desk Host.app's executable: starts the host (~/.makepad/deskhost/bin/deskhost) as its child and waits
// for it, passing on termination signals and its exit status.
//
// Why a launcher: macOS keeps the Screen Recording and Accessibility grants for the app's code
// identity. This launcher is built once per Mac and never changes, so its identity, and the grants,
// stay. The host it starts is rebuilt freely: a child process asks macOS with its parent's grants.
#include <errno.h>
#include <signal.h>
#include <spawn.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/wait.h>
#include <unistd.h>

extern char **environ;
static volatile pid_t child = 0;

static void forward(int sig) {
    if (child > 0) kill(child, sig);
}

int main(int argc, char **argv) {
    (void)argc;
    const char *home = getenv("HOME");
    if (!home) return 1;
    char path[4096];
    snprintf(path, sizeof path, "%s/.makepad/deskhost/bin/deskhost", home);
    signal(SIGTERM, forward);
    signal(SIGINT, forward);
    signal(SIGHUP, forward);
    argv[0] = path;
    pid_t pid;
    int err = posix_spawn(&pid, path, NULL, NULL, argv, environ);
    if (err != 0) {
        fprintf(stderr, "Desk Host: cannot start %s (error %d)\n", path, err);
        return 1;
    }
    child = pid;
    int status = 0;
    while (waitpid(pid, &status, 0) < 0) {
        if (errno != EINTR) return 1;
    }
    if (WIFEXITED(status)) return WEXITSTATUS(status);
    return 128 + WTERMSIG(status);
}
