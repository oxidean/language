/* lang runtime — v0 implementation. See docs/semantics/. */
#include "rt.h"
#include <arpa/inet.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
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
    s->arena_buf = malloc(ARENA_CAP);
    s->arena_cap = ARENA_CAP;
    tls_scope = s;
}

static void *task_wrap(void *p) {
    void *(*fn)(void *) = ((void **)p)[0];
    void *arg = ((void **)p)[1];
    RtScope *s = ((void **)p)[2];
    free(p);
    tls_scope = s;
    fn(arg);
    return 0;
}

void rt_spawn(void *(*fn)(void *), void *arg) {
    RtScope *s = tls_scope;
    if (!s) { /* spawn outside scope: detached */
        pthread_t t;
        void **box = malloc(3 * sizeof(void *));
        box[0] = fn; box[1] = arg; box[2] = NULL;
        if (pthread_create(&t, NULL, task_wrap, box) == 0)
            pthread_detach(t);
        return;
    }
    if (s->n == s->cap) {
        s->cap = s->cap ? s->cap * 2 : 8;
        s->threads = realloc(s->threads, s->cap * sizeof(pthread_t));
    }
    void **box = malloc(3 * sizeof(void *));
    box[0] = fn; box[1] = arg; box[2] = s;
    if (pthread_create(&s->threads[s->n], NULL, task_wrap, box) == 0)
        s->n++;
}

/* cooperative cancel check — tasks observe at suspend points */
static int cancelled(void) { return tls_scope && tls_scope->cancel; }

static int join_scope(RtScope *s) {
    for (int i = 0; i < s->n; i++) pthread_join(s->threads[i], NULL);
    free(s->threads);
    free(s->arena_buf); /* bulk-free: the arena story */
    tls_scope = s->parent;
    return s->trapped ? (int)s->err : 0;
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
    s->trapped = 1;
    s->cancel = 1; /* siblings observe + unwind */
    s->err = code;
    pthread_exit(0);
}

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
        pthread_cond_signal(&ch->cv_recv);
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
    pthread_cond_signal(&ch->cv_send);
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
void rt_println_str(LangStr s) { wr(s); wr(rt_str("\n")); }
void rt_print_i64(int64_t v) { printf("%lld", (long long)v); }
void rt_println_i64(int64_t v) { printf("%lld\n", (long long)v); }

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

/* ---------------- sqlite3 shims ---------------- */
/* v0 FFI can't express T** out-params; these wrap the three calls the
 * pilots need. sqlite3* itself crosses as a plain owned pointer. */

#include <sqlite3.h>

void *rt_sqlite_open(const char *path) {
    sqlite3 *db = NULL;
    if (sqlite3_open(path, &db) != SQLITE_OK) return NULL;
    return db;
}
int64_t rt_sqlite_exec(void *db, const char *sql) {
    return sqlite3_exec((sqlite3 *)db, sql, NULL, NULL, NULL);
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
