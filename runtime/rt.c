/* lang runtime — v0 implementation. See docs/semantics/. */
#include "rt.h"
#include <arpa/inet.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

#define ARENA_CAP (1 << 20)

/* ---------------- allocation & checks ---------------- */

static _Thread_local RtScope *tls_scope;

static ObjHdr *hdr_of(void *obj) { return ((ObjHdr *)obj) - 1; }

static void *alloc_in(RtScope *s, int64_t size, int own) {
    size = (size + 15) & ~15LL;
    int64_t total = size + sizeof(ObjHdr);
    ObjHdr *h;
    if (!own && s && s->arena_buf && s->arena_off + total <= s->arena_cap) {
        h = (ObjHdr *)(s->arena_buf + s->arena_off);
        s->arena_off += total;
        h->arena = 1;
    } else {
        h = malloc(total);
        h->arena = 0;
    }
    static uint32_t gen = 1;
    h->gen = __sync_fetch_and_add(&gen, 1);
    h->borrow = 0;
    h->poisoned = 0;
    return h + 1;
}

void *rt_alloc(int64_t size) { return alloc_in(tls_scope, size, 0); }
void *rt_alloc_own(int64_t size) { return alloc_in(tls_scope, size, 1); }

void rt_free(void *obj) {
    if (!obj) return;
    ObjHdr *h = hdr_of(obj);
    if (!h->arena) free(h);
    /* arena blocks free with their scope */
}

void rt_borrow_read(void *obj) {
    if (!obj) return;
    ObjHdr *h = hdr_of(obj);
    if (h->poisoned || h->borrow == 2) rt_trap(2); /* aliasing */
    h->borrow = 1;
}

void rt_borrow_mut(void *obj) {
    if (!obj) return;
    ObjHdr *h = hdr_of(obj);
    if (h->poisoned || h->borrow != 0) rt_trap(2); /* aliasing */
    h->borrow = 2;
}

void rt_unborrow(void *obj) {
    if (!obj) return;
    hdr_of(obj)->borrow = 0;
}

/* ---------------- scopes, tasks, traps ---------------- */

void rt_scope_enter(RtScope *s) {
    memset(s, 0, sizeof *s);
    s->parent = tls_scope;
    pthread_mutex_init(&s->mu, NULL);
    pthread_cond_init(&s->cv, NULL);
    s->arena_buf = malloc(ARENA_CAP);
    s->arena_cap = ARENA_CAP;
    tls_scope = s;
}

/* ---- interim executor: work pool + blocking pool ----
 * Work pool: N=nproc workers on a shared queue. Blocking pool: a
 * thread per task (FFI calls may block indefinitely). Both decrement
 * scope pending; the state-machine executor keeps the same
 * pending-count join protocol. */

typedef struct Task {
    void *(*fn)(void *);
    void *arg;
    RtScope *scope;
    struct Task *next;
} Task;

static struct {
    pthread_mutex_t mu;
    pthread_cond_t cv;
    Task *head, *tail;
    int started;
} pool = { PTHREAD_MUTEX_INITIALIZER, PTHREAD_COND_INITIALIZER, 0, 0, 0 };

static void scope_done(RtScope *s) {
    if (!s) return;
    pthread_mutex_lock(&s->mu);
    s->pending--;
    if (s->pending <= 0) pthread_cond_signal(&s->cv);
    pthread_mutex_unlock(&s->mu);
}

/* the currently-running task — rt_trap must settle its pending count
 * before pthread_exit, or the scope join waits forever */
static _Thread_local Task *tls_task;

static void run_task(Task *t) {
    tls_scope = t->scope;
    tls_task = t;
    t->fn(t->arg);
    tls_task = NULL;
    scope_done(t->scope);
    free(t);
}

static void *worker(void *_p) {
    (void)_p;
    for (;;) {
        pthread_mutex_lock(&pool.mu);
        while (!pool.head) pthread_cond_wait(&pool.cv, &pool.mu);
        Task *t = pool.head;
        pool.head = t->next;
        if (!pool.head) pool.tail = NULL;
        pthread_mutex_unlock(&pool.mu);
        if (!t->scope || !t->scope->cancel) run_task(t);
        else { scope_done(t->scope); free(t); }
    }
    return 0;
}

static void pool_start(void) {
    pthread_mutex_lock(&pool.mu);
    if (!pool.started) {
        long n = sysconf(_SC_NPROCESSORS_ONLN);
        if (n < 2) n = 2;
        if (n > 32) n = 32;
        for (long i = 0; i < n; i++) {
            pthread_t t;
            if (pthread_create(&t, NULL, worker, NULL) == 0) pthread_detach(t);
        }
        pool.started = 1;
    }
    pthread_mutex_unlock(&pool.mu);
}

static void *task_wrap(void *p) {
    Task *t = p;
    run_task(t);
    return 0;
}

void rt_spawn(void *(*fn)(void *), void *arg) {
    RtScope *s = tls_scope;
    if (s) {
        pthread_mutex_lock(&s->mu);
        s->pending++;
        pthread_mutex_unlock(&s->mu);
    }
    if (!s) { /* outside scope: dedicated thread, nothing to join */
        Task *t = malloc(sizeof *t);
        *t = (Task){ fn, arg, NULL, NULL };
        pthread_t th;
        if (pthread_create(&th, NULL, task_wrap, t) == 0) pthread_detach(th);
        return;
    }
    pool_start();
    Task *t = malloc(sizeof *t);
    *t = (Task){ fn, arg, s, NULL };
    pthread_mutex_lock(&pool.mu);
    if (pool.tail) pool.tail->next = t; else pool.head = t;
    pool.tail = t;
    pthread_cond_signal(&pool.cv);
    pthread_mutex_unlock(&pool.mu);
}

/* blocking pool: tasks that may block in FFI run on dedicated threads --
 * a blocked worker can never starve the work pool */
void rt_spawnb(void *(*fn)(void *), void *arg) {
    RtScope *s = tls_scope;
    if (s) {
        pthread_mutex_lock(&s->mu);
        s->pending++;
        pthread_mutex_unlock(&s->mu);
    }
    Task *t = malloc(sizeof *t);
    *t = (Task){ fn, arg, s, NULL };
    pthread_t th;
    if (pthread_create(&th, NULL, task_wrap, t) == 0) pthread_detach(th);
}

/* cooperative cancel check -- tasks observe at suspend points */
static int cancelled(void) { return tls_scope && tls_scope->cancel; }

static int join_scope(RtScope *s) {
    pthread_mutex_lock(&s->mu);
    while (s->pending > 0) pthread_cond_wait(&s->cv, &s->mu);
    int err = s->trapped ? (int)s->err : 0;
    pthread_mutex_unlock(&s->mu);
    pthread_mutex_destroy(&s->mu);
    pthread_cond_destroy(&s->cv);
    free(s->arena_buf); /* bulk-free: the arena story */
    tls_scope = s->parent;
    return err;
}

void rt_scope_leave(RtScope *s) {
    int err = join_scope(s);
    if (err) {
        if (s->parent) s->parent->trapped = err; /* propagate */
        else { fprintf(stderr, "trap %d escaped top scope\n", err); abort(); }
    }
}

int rt_scope_try(RtScope *s) { return join_scope(s); }

void rt_trap(int64_t code) {
    RtScope *s = tls_scope;
    if (!s) { fprintf(stderr, "trap %lld outside scope\n", (long long)code); abort(); }
    if (!tls_task) {
        /* trap on a non-task thread (main/foreign) — no joiner exists to
         * consume it; fail loudly instead of pthread_exit's silent exit-0 */
        fprintf(stderr, "trap %lld\n", (long long)code);
        abort();
    }
    pthread_mutex_lock(&s->mu);
    if (!s->trapped) { /* first trap wins */
        s->trapped = 1;
        s->err = code;
    }
    s->cancel = 1; /* siblings observe + unwind */
    pthread_mutex_unlock(&s->mu);
    if (tls_task) { /* settle this task's pending slot before dying */
        Task *t = tls_task;
        tls_task = NULL;
        scope_done(t->scope);
        free(t);
    }
    pthread_exit(0);
}

/* ---------------- Shared<T>: atomic word cell ---------------- */

RtShared *rt_mkshared(int64_t v) {
    RtShared *s = malloc(sizeof *s);
    s->val = v;
    s->writer = 0;
    return s;
}

int64_t rt_sget(RtShared *s) {
    return __sync_fetch_and_add(&s->val, 0); /* atomic read */
}

void rt_sset(RtShared *s, int64_t v) {
    __sync_lock_test_and_set(&s->val, v);
    __sync_synchronize();
}

/* CAS — returns the value the cell held before */
int64_t rt_scas(RtShared *s, int64_t expected, int64_t v) {
    return __sync_val_compare_and_swap(&s->val, expected, v);
}

/* atomic add — returns the value before the add */
int64_t rt_sadd(RtShared *s, int64_t delta) {
    return __sync_fetch_and_add(&s->val, delta);
}

/* ---------------- time ---------------- */
int64_t rt_now(void) { return (int64_t)time(NULL); }
int64_t rt_now_ms(void) {
    struct timespec ts;
    clock_gettime(CLOCK_REALTIME, &ts);
    return (int64_t)ts.tv_sec * 1000 + ts.tv_nsec / 1000000;
}
void rt_usleep(int64_t usecs) { usleep((useconds_t)usecs); }

/* ---------------- channels (unbuffered, word payload) ---------------- */

struct RtChan {
    pthread_mutex_t mu;
    pthread_cond_t cv_send, cv_recv;
    int64_t val;
    int has, closed;
};

RtChan *rt_mkchan(void) {
    RtChan *c = calloc(1, sizeof *c);
    pthread_mutex_init(&c->mu, NULL);
    pthread_cond_init(&c->cv_send, NULL);
    pthread_cond_init(&c->cv_recv, NULL);
    return c;
}

void rt_send(RtChan *ch, int64_t v) {
    if (cancelled()) pthread_exit(0);
    pthread_mutex_lock(&ch->mu);
    while (ch->has && !ch->closed) pthread_cond_wait(&ch->cv_send, &ch->mu);
    if (!ch->closed) {
        ch->val = v;
        ch->has = 1;
        pthread_cond_broadcast(&ch->cv_recv);
        while (ch->has && !ch->closed) pthread_cond_wait(&ch->cv_send, &ch->mu);
    }
    pthread_mutex_unlock(&ch->mu);
}

int64_t rt_recv(RtChan *ch) {
    if (cancelled()) pthread_exit(0);
    pthread_mutex_lock(&ch->mu);
    while (!ch->has && !ch->closed) pthread_cond_wait(&ch->cv_recv, &ch->mu);
    int64_t v = ch->val;
    ch->has = 0;
    /* broadcast, not signal: with racing senders a lone wakeup can land
     * on the *completing* send instead of the queued one and strand it */
    pthread_cond_broadcast(&ch->cv_send);
    pthread_mutex_unlock(&ch->mu);
    return v;
}

void rt_close(RtChan *ch) {
    pthread_mutex_lock(&ch->mu);
    ch->closed = 1;
    pthread_cond_broadcast(&ch->cv_send);
    pthread_cond_broadcast(&ch->cv_recv);
    pthread_mutex_unlock(&ch->mu);
}

/* ---------------- strings / misc ---------------- */

LangStr rt_str(const char *s) { return (LangStr){ s, (int64_t)strlen(s) }; }
LangStr rt_str_lit(const char *s) { return rt_str(s); }

LangStr rt_strcat(LangStr a, LangStr b) {
    char *p = rt_alloc(a.n + b.n + 1);
    memcpy(p, a.p, a.n);
    memcpy(p + a.n, b.p, b.n);
    p[a.n + b.n] = 0;
    return (LangStr){ p, a.n + b.n };
}

int rt_streq(LangStr a, LangStr b) {
    return a.n == b.n && memcmp(a.p, b.p, (size_t)a.n) == 0;
}

LangStr rt_itoa(int64_t v) {
    char *p = rt_alloc(24);
    snprintf(p, 24, "%lld", (long long)v);
    return rt_str(p);
}

const char *rt_cstr(LangStr s) {
    char *p = rt_alloc(s.n + 1);
    memcpy(p, s.p, s.n);
    p[s.n] = 0;
    return p;
}

static void wr(LangStr s) { fwrite(s.p, 1, (size_t)s.n, stdout); }
void rt_print_str(LangStr s) { wr(s); }
void rt_println_str(LangStr s) { wr(s); wr(rt_str("\n")); fflush(stdout); }
void rt_print_i64(int64_t v) { printf("%lld", (long long)v); }
void rt_println_i64(int64_t v) { printf("%lld\n", (long long)v); fflush(stdout); }

void rt_sleep(int64_t secs) {
    if (cancelled()) pthread_exit(0);
    sleep((unsigned)secs);
}

/* ---------------- args, strings, buffers ---------------- */

static int g_argc;
static char **g_argv;
void rt_set_args(int argc, char **argv) { g_argc = argc; g_argv = argv; }
int64_t rt_argc(void) { return g_argc; }
LangStr rt_argv(int64_t i) {
    if (i < 0 || i >= g_argc) return rt_str("");
    return rt_str(g_argv[i]);
}

int64_t rt_len(LangStr s) { return s.n; }
int64_t rt_byteat(LangStr s, int64_t i) {
    if (i < 0 || i >= s.n) return -1;
    return (uint8_t)s.p[i];
}
LangStr rt_substr(LangStr s, int64_t a, int64_t b) {
    if (a < 0) a = 0;
    if (b > s.n) b = s.n;
    if (a >= b) return rt_str("");
    char *p = rt_alloc(b - a + 1);
    memcpy(p, s.p + a, b - a);
    p[b - a] = 0;
    return (LangStr){ p, b - a };
}
int64_t rt_find(LangStr s, LangStr sub) {
    for (int64_t i = 0; i + sub.n <= s.n; i++)
        if (memcmp(s.p + i, sub.p, (size_t)sub.n) == 0) return i;
    return -1;
}
int64_t rt_atoi(LangStr s) { return strtoll(s.p, NULL, 10); }

void *rt_mkbuf(int64_t size) { return rt_alloc_own(size); }
LangStr rt_bufstr(void *buf, int64_t n) {
    char *p = rt_alloc(n + 1);
    memcpy(p, buf, (size_t)n);
    p[n] = 0;
    return (LangStr){ p, n };
}
void rt_poke8(void *buf, int64_t off, int64_t v) { ((uint8_t *)buf)[off] = (uint8_t)v; }
void rt_poke16(void *buf, int64_t off, int64_t v) { ((uint16_t *)buf)[off / 2] = htons((uint16_t)v); }
void rt_poke32(void *buf, int64_t off, int64_t v) { ((uint32_t *)buf)[off / 4] = htonl((uint32_t)v); }
int64_t rt_htons(int64_t v) { return htons((uint16_t)v); }
int64_t rt_htonl(int64_t v) { return htonl((uint32_t)v); }

/* ---------------- file io + process ---------------- */

LangStr rt_readfile(LangStr path) {
    char *cp = rt_alloc(path.n + 1);
    memcpy(cp, path.p, (size_t)path.n);
    cp[path.n] = 0;
    FILE *f = fopen(cp, "rb");
    if (!f) return rt_str("");
    fseek(f, 0, SEEK_END);
    long n = ftell(f);
    fseek(f, 0, SEEK_SET);
    char *buf = rt_alloc(n + 1);
    fread(buf, 1, (size_t)n, f);
    fclose(f);
    buf[n] = 0;
    return (LangStr){ buf, n };
}

int64_t rt_writefile(LangStr path, LangStr data) {
    char *cp = rt_alloc(path.n + 1);
    memcpy(cp, path.p, (size_t)path.n);
    cp[path.n] = 0;
    FILE *f = fopen(cp, "wb");
    if (!f) return -1;
    fwrite(data.p, 1, (size_t)data.n, f);
    fclose(f);
    return 0;
}

void rt_exit(int64_t code) { exit((int)code); }

/* ---------------- sqlite3 shims ---------------- */
/* v0 FFI can't express T** out-params; these wrap the three calls the
 * pilots need. sqlite3* itself crosses as a plain owned pointer. */

#include <sqlite3.h>

void *rt_sqlite_open(const char *path) {
    sqlite3 *db = NULL;
    if (sqlite3_open(path, &db) != SQLITE_OK) return NULL;
    sqlite3_busy_timeout(db, 5000); /* serialize concurrent writers */
    sqlite3_exec(db, "PRAGMA journal_mode=WAL", NULL, NULL, NULL);
    return db;
}
int64_t rt_sqlite_exec(void *db, const char *sql) {
    /* busy_timeout doesn't retry SQLITE_BUSY_SNAPSHOT; a bounded retry
     * loop covers both busy flavors under concurrent writers */
    for (int i = 0; i < 500; i++) {
        int rc = sqlite3_exec((sqlite3 *)db, sql, NULL, NULL, NULL);
        if (rc != SQLITE_BUSY && rc != SQLITE_LOCKED) {
            return rc;
        }
        usleep(10000);
    }
    return SQLITE_BUSY;
}
LangStr rt_sqlite_query(void *db, const char *sql) {
    sqlite3_stmt *st = NULL;
    LangStr out = rt_str("");
    if (sqlite3_prepare_v2((sqlite3 *)db, sql, -1, &st, NULL) != SQLITE_OK)
        return out;
    while (sqlite3_step(st) == SQLITE_ROW) {
        int cols = sqlite3_column_count(st);
        for (int c = 0; c < cols; c++) {
            const unsigned char *t = sqlite3_column_text(st, c);
            out = rt_strcat(out, rt_str(t ? (const char *)t : ""));
            if (c + 1 < cols) out = rt_strcat(out, rt_str("|"));
        }
        out = rt_strcat(out, rt_str("\n"));
    }
    sqlite3_finalize(st);
    return out;
}
void rt_sqlite_close(void *db) { sqlite3_close((sqlite3 *)db); }

/* ---------------- entry ---------------- */

int main(int argc, char **argv) {
    rt_set_args(argc, argv);
    RtScope base;
    rt_scope_enter(&base);
    lang_main();
    rt_scope_leave(&base);
    return 0;
}
