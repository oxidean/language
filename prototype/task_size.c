/* Prototype: per-task memory — stackless state-machine frame vs OS thread.
 *
 * Models a suspend-capable function with 3 suspend points and a handful of
 * live locals as (a) a compiled state-machine frame and (b) a pthread.
 * Reports static sizes plus measured RSS for N live "tasks" of each kind.
 */
#include <pthread.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>
#include <sched.h>

#define N_TASKS 2048

/* ---- stackless: what the state-machine transform would emit ---- */

typedef enum { SM_S0, SM_S1, SM_S2, SM_DONE } SmState;

typedef struct {
    SmState pc;            /* resume point */
    int32_t gen;           /* ObjHdr.gen (ownership check) */
    uint8_t borrow;        /* ObjHdr.borrow */
    uint8_t arena;         /* ObjHdr.arena */
    /* live locals lifted into the frame */
    int64_t acc;
    void *buf;
    int64_t i;
    void *chan;
} TaskFrame;

typedef int (*StepFn)(TaskFrame *);

typedef struct {
    TaskFrame f;
    StepFn step;
} Task;

static long rss_kb(void) {
    long v = 0;
    FILE *f = fopen("/proc/self/status", "r");
    char line[256];
    while (f && fgets(line, sizeof line, f))
        if (sscanf(line, "VmRSS: %ld kB", &v) == 1) break;
    if (f) fclose(f);
    return v;
}

/* arrived count replaces a fixed-count barrier: a partial pthread_create
 * run must not leave main waiting on threads that never spawned */
static volatile int arrived;
static void *idle(void *arg) {
    (void)arg;
    __sync_fetch_and_add(&arrived, 1);
    for (;;) pause();
    return NULL;
}

int main(void) {
    printf("TaskFrame (state machine):  %zu bytes\n", sizeof(TaskFrame));
    printf("Task (frame + step fn):     %zu bytes\n", sizeof(Task));

    pthread_attr_t attr;
    pthread_attr_init(&attr);
    size_t stack = 0;
    pthread_attr_getstacksize(&attr, &stack);
    printf("pthread default stack:      %zu bytes\n", stack);

    /* N stackless tasks */
    long before = rss_kb();
    Task *tasks = calloc(N_TASKS, sizeof(Task));
    for (int i = 0; i < N_TASKS; i++) tasks[i].f.acc = i;
    long sm_rss = rss_kb() - before;
    printf("%d stackless tasks RSS:    %+ld kB (~%ld B/task)\n",
           N_TASKS, sm_rss, sm_rss * 1024 / N_TASKS);
    free(tasks);

    /* N OS-thread tasks */
    pthread_t *th = malloc(N_TASKS * sizeof(pthread_t));
    before = rss_kb();
    int spawned = N_TASKS;
    for (int i = 0; i < N_TASKS; i++)
        if (pthread_create(&th[i], &attr, idle, NULL)) {
            fprintf(stderr, "pthread_create failed at %d\n", i);
            spawned = i;
            break;
        }
    while (__sync_fetch_and_add(&arrived, 0) < spawned) sched_yield();
    long th_rss = rss_kb() - before;
    printf("%d thread tasks RSS:       %+ld kB (~%ld B/task)\n",
           spawned, th_rss, spawned ? th_rss * 1024 / spawned : 0);

    /* a partial spawn makes the totals incommensurate — only a full
     * run of N_TASKS threads yields a meaningful ratio */
    if (spawned == N_TASKS && sm_rss > 0 && th_rss > 0)
        printf("ratio:                    %.0fx\n", (double)th_rss / sm_rss);
    return 0;
}
