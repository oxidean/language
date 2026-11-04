/* oxl runtime — public surface for generated code.
 * Implements the contracts in docs/semantics/{ownership,concurrency}.md:
 * ObjHdr checks, scope arenas, scope-contained traps, task executor,
 * unbuffered ownership-transfer channels.
 */
#pragma once
#include <pthread.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>

typedef struct {
    const char *p;
    int64_t n;
} OxlStr;

typedef struct RtScope RtScope;
typedef struct ObjHdr {
    uint32_t gen;
    uint8_t borrow; /* 0 free, 1 read-held, 2 mut-held */
    uint8_t arena;
    uint8_t poisoned;
    uint8_t _pad;
    /* heap-fallback tracking: non-own blocks that miss the arena are
     * still scope-lived — chained on the scope's `heap` list and freed
     * at join. hscope links back for rt_free's unlink. */
    RtScope *hscope;
    struct ObjHdr *hnext;
} ObjHdr;

/* --- frames & tasks ---
 * Suspendable fns compile to a heap frame (params + live locals +
 * resume counter) and a poll driver. Tasks carry a frame, not a stack;
 * the executor calls poll until it returns != PENDING. */
typedef struct RtTask RtTask;
typedef struct RtFrame {
    struct RtScope *cur_scope; /* innermost live scope at last suspend */
    int state;                 /* resume counter: 0 = entry */
} RtFrame;
typedef int (*RtPoll)(RtFrame *, RtTask *);
#define RT_READY 0   /* poll result: fn returned (result slot set) */
#define RT_PENDING 1 /* suspended; the waker re-enqueues the task */
#define RT_CANCEL 2  /* observed scope cancel at a suspend point */

/* --- scopes & tasks (concurrency.md) ---
 * Executor: fixed worker pool driving frame tasks on a shared MPMC
 * queue; blocking FFI still gets thread-per-task. Scopes join by
 * pending-task count — join is itself a suspend point for frames. */
struct RtScope {
    volatile int pending;
    volatile int trapped;
    volatile int cancel;
    int64_t err;
    struct RtScope *parent;
    pthread_mutex_t mu;
    pthread_cond_t cv;
    RtTask *join_task; /* frame suspended at join, if any */
    RtTask *parked;    /* tasks suspended inside this scope (pnext chain) */
    /* inner scopes live in the owning task's frame — if the owner dies
     * mid-block (trap/cancel) the scope is orphaned: the last child to
     * settle runs the teardown join_scope would have, then releases
     * the frame */
    RtTask *owner;
    volatile int orphan;
    /* arena: bump allocator freed on leave */
    char *arena_buf;
    int64_t arena_off, arena_cap;
    ObjHdr *heap;      /* non-own blocks that missed the arena (freed at join) */
};

/* the calling thread's innermost scope — frame drivers save/restore it
 * around poll() so interleaved tasks on one worker keep their own chain */
extern _Thread_local RtScope *tls_scope;

void rt_scope_enter(RtScope *s);
void rt_scope_leave(RtScope *s);
int rt_scope_try(RtScope *s); /* join; return trap code without propagating */
int rt_scope_join_poll(RtScope *s, RtTask *t); /* frame join: READY/PENDING */
int rt_scope_finish(RtScope *s);              /* post-join teardown -> err */
void rt_spawn(void *(*fn)(void *), void *arg);   /* fn task (non-susp fns) */
void rt_spawnb(void *(*fn)(void *), void *arg);  /* blocking pool (FFI) */
RtFrame *rt_task_new(RtPoll poll, int64_t bytes); /* one alloc: task+frame */
void rt_spawn_task(RtFrame *fr);                  /* enqueue frame task in scope */
void rt_spawnb_task(RtFrame *fr);                 /* frame task, blocking pool */
int rt_run_root(RtFrame *fr);                     /* drive a task to completion */
void rt_task_wake(RtTask *t);                    /* re-enqueue a suspended task */
int rt_cancelled(void);
int64_t rt_task_data(RtTask *t);                 /* delivered channel value */
void rt_trap(int64_t code);

/* --- defer: runtime registration stack, per task ---
 * `defer f(x)` pushes {thunk, captured env} at statement execution; fn
 * exits drain down to their entry mark (LIFO). The stack lives on the
 * task so entries survive suspends; trap/cancel unwinds drain whatever
 * remains before the task settles. */
typedef struct RtDefer {
    void (*fp)(void *);
    void *env;
    struct RtDefer *next;
} RtDefer;
RtDefer *rt_defer_mark(void);
void rt_defer_push(void (*fp)(void *), void *env);
void rt_defer_drain_to(RtDefer *mark);

/* --- Shared<T>: atomic word cell for cross-scope sharing --- */
typedef struct {
    volatile int64_t val;
    volatile int32_t writer; /* 1 while a writer holds it */
} RtShared;
RtShared *rt_mkshared(int64_t v);
int64_t rt_sget(RtShared *s);
void rt_sset(RtShared *s, int64_t v);
int64_t rt_scas(RtShared *s, int64_t expected, int64_t v); /* CAS → old */
int64_t rt_sadd(RtShared *s, int64_t delta);               /* fetch+add → old */
int64_t rt_now(void);          /* unix seconds */
int64_t rt_now_ms(void);       /* unix ms */
void rt_usleep(int64_t usecs);

/* --- allocation & ownership checks (ownership.md) --- */
void *rt_alloc(int64_t size);       /* scope arena if inside scope, else heap */
void *rt_alloc_own(int64_t size);   /* always heap (escaping data) */
void rt_borrow_read(void *obj);
void rt_borrow_mut(void *obj);
void rt_unborrow(void *obj);
void rt_free(void *obj);

/* --- channels ---
 * Two surfaces on one chan: blocking rt_send/rt_recv for thread tasks,
 * *_poll variants for frame tasks (READY = completed; PENDING = the
 * task was parked on the chan's wait queue and will be re-enqueued). */
typedef struct RtChan RtChan;
RtChan *rt_mkchan(int64_t cap);
void rt_send(RtChan *ch, int64_t v);
int64_t rt_recv(RtChan *ch);
int rt_send_poll(RtChan *ch, int64_t v, RtTask *t);
int rt_recv_poll(RtChan *ch, RtTask *t);
void rt_close(RtChan *ch);
int rt_sleep_poll(int64_t secs, RtTask *t);

/* --- select: park on N chans at once; first ready arm wins ---
 * sel_arm values: 0..n-1 = chan slot, RT_SEL_TMO = timeout, RT_SEL_ELSE */
#define RT_SEL_TMO (-2)
#define RT_SEL_ELSE (-3)
int rt_select_poll(RtChan **chs, int n, int64_t tmo_ms, int has_else, RtTask *t);
int64_t rt_select_arm(RtTask *t); /* cleanup parked waiters; returns arm */

/* --- slices: {ptr,len,cap,heap} fat header, erased element type ---
 * borrow-slice backing comes from rt_alloc (scope arena when inside a
 * scope) and dies with the scope; `heap` marks backings that must
 * outlive scopes (own slices) — views inherit it and growth preserves
 * it, so an own slice never follows a freed arena pointer. */
typedef struct {
    void *ptr;
    int64_t len;
    int64_t cap;
    int64_t heap;
} RtSlice;
RtSlice rt_mkslice(int64_t n, int64_t esz);            /* len==cap==n, zeroed */
RtSlice rt_slice_push(RtSlice s, int64_t esz, void *v); /* grow + append */
RtSlice *rt_slice_own(RtSlice s, int64_t esz);          /* deep-copy to heap: own []T */

/* --- function values (B4): {env, fnptr} fat pair. The callee
 * signature is always (void* env, args...) — named fns get a
 * zero-env wrapper so every call goes through the same shape */
typedef struct {
    void *env;
    void *fp;
} RtFnv;

/* --- interface values (D3): {data, vtable} fat pair, Go-style itab.
 * vtab[i] is the fn ptr for the interface's i-th method; its signature
 * is always (void* data, args...) so any impl slots in uniformly */
typedef struct {
    void *data;
    void **vtab;
} RtIface;

/* --- misc builtins --- */
void rt_sleep(int64_t secs);
OxlStr rt_str_lit(const char *s);
OxlStr rt_strcat(OxlStr a, OxlStr b);
int rt_streq(OxlStr a, OxlStr b);
OxlStr rt_itoa(int64_t v);
OxlStr rt_str(const char *s);
void rt_print_str(OxlStr s);
void rt_print_i64(int64_t v);
void rt_println_str(OxlStr s);
void rt_println_i64(int64_t v);
void rt_print_f64(double v);
void rt_println_f64(double v);
const char *rt_cstr(OxlStr s);

/* args + strings + byte buffers (stdlib wave-1 surface) */
void rt_set_args(int argc, char **argv);
int64_t rt_argc(void);
OxlStr rt_argv(int64_t i);
int64_t rt_len(OxlStr s);
int64_t rt_byteat(OxlStr s, int64_t i);
OxlStr rt_substr(OxlStr s, int64_t a, int64_t b);
int64_t rt_find(OxlStr s, OxlStr sub);
int64_t rt_atoi(OxlStr s);
double rt_atof(OxlStr s);
OxlStr rt_ftoa(double v);
void *rt_mkbuf(int64_t size);
OxlStr rt_bufstr(void *buf, int64_t n);
void rt_poke8(void *buf, int64_t off, int64_t v);
void rt_poke16(void *buf, int64_t off, int64_t v);
void rt_poke32(void *buf, int64_t off, int64_t v);
int64_t rt_set_rcvtimo(int64_t fd, int64_t secs);      /* SO_RCVTIMEO, portable */
int64_t rt_htons(int64_t v);
int64_t rt_htonl(int64_t v);
OxlStr rt_readfile(OxlStr path);
int64_t rt_writefile(OxlStr path, OxlStr data);
void rt_exit(int64_t code);

/* sqlite3 shims — out-param (T**) FFI needs these until the type
 * system gains address-of; direct calls (strlen etc.) use `extern` */
void *rt_sqlite_open(const char *path);
int64_t rt_sqlite_exec(void *db, const char *sql);
int64_t rt_sqlite_exec_bind(void *db, const char *sql, OxlStr param); /* bind ?1 */
OxlStr rt_sqlite_query(void *db, const char *sql);
void rt_sqlite_close(void *db);

/* entry: generated code provides oxl_main() */
void oxl_main(void);
