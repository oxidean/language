/* Prototype: dynamic ownership-check overhead.
 *
 * Tight loop over an array of headered objects: unchecked raw access vs
 * access through the borrow/gen checks the runtime would emit. Reports
 * ns/op for each. The check is two loads + two predictable branches.
 */
#define _POSIX_C_SOURCE 199309L
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <time.h>

#define N_OBJS 4096
#define N_ITERS 2000

typedef struct {
    uint32_t gen;
    uint8_t borrow; /* 0 free, 1 shared, 2 mut */
    uint8_t arena;
    uint8_t pad[2];
    int64_t payload;
} Obj;

static int traps;

static inline int64_t read_unchecked(Obj *o) { return o->payload; }

static inline int64_t read_checked(Obj *o, uint32_t gen) {
    if (__builtin_expect(o->gen != gen, 0)) { traps++; return -1; }
    if (__builtin_expect(o->borrow == 2, 0)) { traps++; return -1; }
    return o->payload;
}

static double bench(int64_t (*fn)(Obj *, uint32_t), Obj *objs) {
    struct timespec a, b;
    volatile int64_t sink = 0;
    clock_gettime(CLOCK_MONOTONIC, &a);
    for (int it = 0; it < N_ITERS; it++)
        for (int i = 0; i < N_OBJS; i++)
            sink += fn(&objs[i], 7);
    clock_gettime(CLOCK_MONOTONIC, &b);
    if (sink == -1) fprintf(stderr, "impossible\n"); /* keep sink live */
    double ns = (b.tv_sec - a.tv_sec) * 1e9 + (b.tv_nsec - a.tv_nsec);
    return ns / ((double)N_ITERS * N_OBJS);
}

static int64_t raw(Obj *o, uint32_t g) { (void)g; return read_unchecked(o); }
static int64_t chk(Obj *o, uint32_t g) { return read_checked(o, g); }

int main(void) {
    Obj *objs = malloc(N_OBJS * sizeof(Obj));
    for (int i = 0; i < N_OBJS; i++) {
        objs[i].gen = 7;
        objs[i].borrow = 0;
        objs[i].payload = i;
    }
    double t_raw = bench(raw, objs);
    double t_chk = bench(chk, objs);
    printf("unchecked access: %.3f ns/op\n", t_raw);
    printf("checked access:   %.3f ns/op\n", t_chk);
    printf("check overhead:   %.3f ns/op (%.1f%%)\n",
           t_chk - t_raw, 100.0 * (t_chk - t_raw) / t_raw);
    printf("sizeof(ObjHdr):   %zu bytes + payload\n",
           sizeof(Obj) - sizeof(int64_t));
    if (traps) fprintf(stderr, "unexpected traps: %d\n", traps);
    free(objs);
    return 0;
}
