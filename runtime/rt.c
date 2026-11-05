/* oxl runtime — v0 implementation. See docs/semantics/. */
#define _DEFAULT_SOURCE /* keep POSIX/BSD decls visible under strict -std */
#include "rt.h"
#ifndef _WIN32
#include <arpa/inet.h>
#endif
#include <errno.h>
#include <setjmp.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#ifndef _WIN32
#include <sys/socket.h>
#endif
#include <time.h>
#ifndef _WIN32
#include <unistd.h>
#endif

#define ARENA_CAP (1 << 20)

/* ---------------- allocation & checks ---------------- */

_Thread_local RtScope *tls_scope;
static _Thread_local RtTask *tls_task;

/* raw-allocation policy: runtime internals abort on OOM like alloc_in —
 * no NULL returns to unchecked call sites */
static void *xmalloc(size_t n) {
    void *p = malloc(n);
    if (!p && n) { fprintf(stderr, "oxl: out of memory (%zu bytes)\n", n); abort(); }
    return p;
}
static void *xcalloc(size_t n, size_t sz) {
    void *p = calloc(n, sz);
    if (!p && n && sz) { fprintf(stderr, "oxl: out of memory (%zu bytes)\n", n * sz); abort(); }
    return p;
}
static void *xrealloc(void *p, size_t n) {
    void *q = realloc(p, n);
    if (!q && n) { fprintf(stderr, "oxl: out of memory (%zu bytes)\n", n); abort(); }
    return q;
}

static ObjHdr *hdr_of(void *obj) { return ((ObjHdr *)obj) - 1; }

static void *alloc_in(RtScope *s, int64_t size, int own) {
    size = (size + 15) & ~15LL;
    int64_t total = size + sizeof(ObjHdr);
    ObjHdr *h;
    /* several tasks can share one scope arena — the bump must be an
     * atomic claim or two allocs can land on the same offset */
    int64_t off = -1;
    if (!own && s && s->arena_buf && s->arena_off + total <= s->arena_cap)
        off = __sync_fetch_and_add(&s->arena_off, total);
    if (off >= 0 && off + total <= s->arena_cap) {
        h = (ObjHdr *)(s->arena_buf + off);
        h->arena = 1;
        h->hscope = NULL;
    } else {
        h = malloc(total);
        if (!h) { fprintf(stderr, "oxl: out of memory (%lld bytes)\n", (long long)total); abort(); }
        h->arena = 0;
        h->hscope = NULL;
        /* non-own allocs that miss the arena are still scope-lived:
         * track them so join_scope frees them with the arena */
        if (!own && s) {
            oxl_mu_lock(&s->mu);
            h->hscope = s;
            h->hnext = s->heap;
            s->heap = h;
            oxl_mu_unlock(&s->mu);
        }
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
    if (!h->arena) {
        if (h->hscope) { /* scope-tracked: unlink before freeing */
            RtScope *s = h->hscope;
            oxl_mu_lock(&s->mu);
            ObjHdr **pp = &s->heap;
            while (*pp) {
                if (*pp == h) { *pp = h->hnext; break; }
                pp = &(*pp)->hnext;
            }
            oxl_mu_unlock(&s->mu);
        }
        free(h);
    }
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

/* serializes scope-tree membership: every parent link write and every
 * children-list traversal runs under link_mu. Children hold no pending
 * ref on their parent, so a parent can tear down under a live child —
 * teardown detaches its children under link_mu BEFORE its mutex dies,
 * and a surviving child's later unlink sees parent==NULL instead of
 * locking destroyed memory. Lock order: link_mu first, at most one
 * scope mu nested inside; never two scope mutexes under it. */
static oxl_mutex_t link_mu = OXL_MUTEX_INIT;

void rt_scope_enter(RtScope *s) {
    memset(s, 0, sizeof *s);
    s->parent = tls_scope;
    oxl_mu_init(&s->mu, NULL);
    oxl_cv_init(&s->cv, NULL);
    s->arena_buf = xmalloc(ARENA_CAP);
    s->arena_cap = ARENA_CAP;
    s->owner = tls_task;
    if (s->parent) {
        /* link into the parent's descendant list so rt_trap can cancel
         * the whole subtree; a scope born under an already-cancelled
         * parent starts cancelled — its first suspend point dies
         * instead of outliving the trap that spawned it. A dead parent
         * (teardown already committed) takes no new children: stay
         * unlinked so our own unlink can't touch freed memory. link_mu
         * held first pins p: teardown can't reach mutex-destroy until
         * it wins unlink, so p->mu is alive for this whole section.
         * The local keeps p valid for the unlock after we detach. */
        RtScope *p = s->parent;
        oxl_mu_lock(&link_mu);
        oxl_mu_lock(&p->mu);
        if (p->cancel || p->dead) s->cancel = 1;
        if (p->dead) s->parent = NULL;
        else {
            s->sib = p->children;
            p->children = s;
        }
        oxl_mu_unlock(&p->mu);
        oxl_mu_unlock(&link_mu);
    }
    tls_scope = s;
}

/* ---- executor: frame-task work pool + blocking pool ----
 * Work pool: N=nproc workers on a shared queue driving frame tasks —
 * each poll() runs a suspendable fn's state machine until it returns
 * READY/PENDING/CANCEL. PENDING tasks are owned by their waker (chan,
 * scope join, timer thread) which re-enqueues them. Blocking pool:
 * a thread per task for FFI/blocking fns. Both decrement scope pending
 * on completion — the join protocol is unchanged. */

/* wakeup cell for tasks driven outside the work pool (root, blocking
 * pool): owned by the driver's stack, pointed at for the task's life */
typedef struct RtWake {
    oxl_mutex_t mu;
    oxl_cond_t cv;
    volatile int ready;
} RtWake;

/* one unwind target per driver thread — a task's setjmp is only valid
 * while its poll is on the C stack, so the target can live in TLS */
static _Thread_local jmp_buf tls_jb;

struct RtTask {
    RtPoll poll;          /* frame tasks: driver fn */
    RtFrame *frame;       /* == (RtFrame *)(t + 1) for rt_task_new blocks */
    void *(*fn)(void *);  /* thread tasks: plain fn */
    void *arg;
    RtScope *scope;
    struct RtTask *next;  /* pool queue / chan wait queue link */
    int is_frame;
    int external;         /* driven by rt_run_root / blocking thread */
    RtWake *wk;           /* external wakeup, driver-stack-owned */
    int64_t chan_res;     /* channel delivery slot */
    /* parked-on-a-waitqueue bookkeeping: scope cancel enumerates the
     * scope's parked tasks and force-wakes them so the join can settle.
     * Wake ownership = CAS parked 1->0; queue unlink is hygiene. */
    volatile int parked;
    RtScope *park_scope;      /* scope whose `parked` list holds us */
    struct RtTask *pnext;     /* parked-list link */
    void *waitobj;            /* RtChan* | RtScope* | RtSel* */
    int waitkind;             /* WK_* below */
    struct RtSel *sel;        /* live select wait, if any */
    int sel_arm;              /* winning select arm */
    /* wake-vs-park protocol (goready/Gopark): ONSTACK while poll runs;
     * a waker swaps PARKED->WAKE_REQ or plants WAKE_REQ for the driver
     * to self-enqueue when it actually leaves the C stack */
    volatile int wstate;
    struct RtDefer *defers;  /* fn-scope defer stack (LIFO) */
    /* one hold per still-open inner scope + one self-hold taken while
     * settling — the frame frees when the last hold drops, so children
     * that outlive their parent's trap never scope_done into dead
     * frame memory */
    volatile int holds;
};
#define WK_RECV 1
#define WK_SEND 2
#define WK_SEL 3
#define WK_JOIN 4
#define WK_TRECV 5
#define WK_TSEND 6
#define WK_SLEEP 7

/* task lifetime: `holds` is the allocation's refcount — one "run" ref
 * taken at spawn and dropped at settle, plus one per queue membership
 * (scope parked list, chan recvq/sendq node, select arm node, the
 * RtSel itself) and per live orphan scope. A waker inherits the ref of
 * whatever node it removes, so claimed-or-not the task stays valid
 * memory across claim+wake; the last dropper frees task+frame. */
static void task_ref(RtTask *t) { __sync_add_and_fetch(&t->holds, 1); }
static void task_unref(RtTask *t) {
    if (__sync_sub_and_fetch(&t->holds, 1) == 0) free(t);
}

/* register `t` as parked on `obj` in scope `s` — callers hold the wait
 * queue's own lock when the enqueue must be atomic with registration.
 * A listed node holds one task ref; whoever unlinks it owns that ref. */
static void park_add(RtTask *t, RtScope *s, void *obj, int kind) {
    if (!t) return;
    if (!s) { t->parked = 1; t->waitobj = obj; t->waitkind = kind; return; }
    task_ref(t);
    oxl_mu_lock(&s->mu);
    t->parked = 1;
    t->park_scope = s;
    t->waitobj = obj;
    t->waitkind = kind;
    t->pnext = s->parked;
    s->parked = t;
    oxl_mu_unlock(&s->mu);
}

/* drop `t` from its scope's parked list (idempotent hygiene; the claim
 * itself is the CAS on `parked`) */
static void park_del(RtTask *t) {
    RtScope *s = t->park_scope;
    if (!s) return;
    int unlinked = 0;
    oxl_mu_lock(&s->mu);
    if (t->pnext != t) { /* parked-list sentinel: self-link = not listed */
        RtTask **pp = &s->parked;
        while (*pp) {
            if (*pp == t) { *pp = t->pnext; unlinked = 1; break; }
            pp = &(*pp)->pnext;
        }
        t->pnext = t;
    }
    oxl_mu_unlock(&s->mu);
    if (unlinked) task_unref(t); /* the list's ref leaves with the node */
}

/* the wake claim: whoever flips parked 1->0 owns the wake */
static int park_claim(RtTask *t) {
    return __sync_bool_compare_and_swap(&t->parked, 1, 0);
}

#define WS_ONSTACK 0
#define WS_PARKED 1
#define WS_WAKEREQ 2

int64_t rt_select_arm(RtTask *t); /* defined with the channel internals */
static void scope_cancel_wake(RtScope *s);
static void pool_push(RtTask *t);
static void pool_start(void);

static struct {
    oxl_mutex_t mu;
    oxl_cond_t cv;
    RtTask *head, *tail;
    int started;
    int workers; /* live detached workers — spawn_fallback checks this */
} pool = { OXL_MUTEX_INIT, OXL_CV_INIT, 0, 0, 0, 0 };

static void scope_heap_free(RtScope *s);

/* teardown for a scope no joiner will consume: the arena bump buffer
 * and the scope-tracked heap blocks. Sync objects die too — pending==0
 * means no thread can still be holding or waiting on them. The scope
 * struct itself is frame (or stack) memory — not freed here. */
/* drop s from its parent's descendant list — idempotent. Membership
 * lives under link_mu, never p->mu: p's teardown may already have run
 * (it NULLs our parent link first under the same mutex), so a NULL
 * here means "already detached — the struct is gone, touch nothing". */
static void scope_unlink(RtScope *s) {
    oxl_mu_lock(&link_mu);
    RtScope *p = s->parent;
    if (p) {
        RtScope **pp = &p->children;
        while (*pp) {
            if (*pp == s) { *pp = s->sib; break; }
            pp = &(*pp)->sib;
        }
    }
    s->parent = NULL;
    s->sib = NULL;
    oxl_mu_unlock(&link_mu);
}

/* detach every child of s under link_mu: each survives with parent==NULL
 * so its later teardown/walk can never lock s's about-to-die mutex.
 * Runs before sync destruction in every teardown path. */
static void scope_detach_children(RtScope *s) {
    oxl_mu_lock(&link_mu);
    for (RtScope *c = s->children; c; ) {
        RtScope *n = c->sib;
        c->parent = NULL;
        c->sib = NULL;
        c = n;
    }
    s->children = NULL;
    oxl_mu_unlock(&link_mu);
}

static void scope_teardown(RtScope *s) {
    oxl_mu_lock(&s->mu);
    s->dead = 1; /* teardown commits here: trap walks must skip us */
    oxl_mu_unlock(&s->mu);
    scope_unlink(s);
    scope_detach_children(s);
    oxl_mu_destroy(&s->mu);
    oxl_cv_destroy(&s->cv);
    scope_heap_free(s);
    free(s->arena_buf);
    s->arena_buf = NULL;
}

/* child-side teardown of an orphaned scope: frees the scope's memory
 * and drops the owner's frame hold — the last dropper frees the frame */
static void scope_orphan_free(RtScope *s) {
    scope_teardown(s);
    RtTask *o = s->owner;
    if (o) task_unref(o);
}

static void scope_done(RtScope *s) {
    if (!s) return;
    RtTask *wake = NULL;
    int orphan_drain = 0;
    oxl_mu_lock(&s->mu);
    s->pending--;
    if (s->pending <= 0) {
        if (s->join_task) wake = s->join_task;
        else if (s->orphan) { orphan_drain = 1; s->dead = 1; }
        else oxl_cv_signal(&s->cv);
    }
    oxl_mu_unlock(&s->mu);
    /* outside s->mu: avoids s->mu/pool.mu inversion. The joiner is ours
     * to wake — but its parked ref can be the last one, and park_del
     * would free it under us: pin it across the wake first. */
    if (orphan_drain) scope_orphan_free(s);
    if (wake && park_claim(wake)) {
        task_ref(wake);
        park_del(wake);
        rt_task_wake(wake);
        task_unref(wake);
    }
}

/* SIGPIPE must not kill the process before write() returns EPIPE —
 * servers ignore it. Wrapped in rt.c because a libc `signal` extern
 * conflicts with the real prototype (clang -Wint-conversion). */
void rt_sigpipe_ignore(void) {
#ifndef _WIN32
    signal(SIGPIPE, SIG_IGN);
#endif
}

/* heap-allocated scope for library code that can't name RtScope's
 * size (stdlib http trap containment). Join with rt_scope_join_free —
 * rt_scope_try semantics: the trap code returns instead of
 * propagating to the parent. */

void *rt_scope_new(void) {
    /* rt_alloc into the CURRENT scope, not raw malloc/own: callers wrap
     * the value in borrow checks (rt_borrow_mut) that need a real
     * ObjHdr, and the trailing rt_unborrow lands after join_free — so
     * the struct must outlive its own join. The enclosing scope reaps
     * the dead struct with its arena; rt_scope_try already tears down
     * the internals (arena_buf, sync, heap list) at join. */
    RtScope *s = rt_alloc(sizeof *s);
    rt_scope_enter(s);
    return s;
}

/* name kept for the extern surface: the "free" is the parent scope
 * reaping the dead struct — the join already did the real teardown */
int64_t rt_scope_join_free(void *sv) {
    return rt_scope_try((RtScope *)sv);
}

/* serializes trap-time descendant passes: trappers share the wnext
 * scratch links, so two racing traps must not clobber each other */
static oxl_mutex_t cancel_tree_mu = OXL_MUTEX_INIT;

/* cancel every scope below s and force-wake their parked tasks.
 * The children list lives under link_mu: while it is held no child can
 * detach or be detached, so c->mu is safe to take (link_mu -> c->mu).
 * A child that already committed teardown (dead) is skipped; a live one
 * gets a temp pending hold so its join/drain can't complete between
 * our unlock and the wake — dropped after its subtree is done. */
static void scope_cancel_tree(RtScope *s) {
    RtScope *list = NULL;
    oxl_mu_lock(&link_mu);
    for (RtScope *c = s->children; c; c = c->sib) {
        oxl_mu_lock(&c->mu);
        if (!c->dead) {
            c->cancel = 1;
            c->pending++;
            c->wnext = list;
            list = c;
        }
        oxl_mu_unlock(&c->mu);
    }
    oxl_mu_unlock(&link_mu);
    while (list) {
        RtScope *c = list;
        list = c->wnext;
        c->wnext = NULL;
        scope_cancel_wake(c);
        scope_cancel_tree(c);
        /* release the temp hold through scope_done: descendants can be
         * non-orphan with a parked joiner, and the pending->0 protocol
         * (wake join_task / drain orphan / signal cv) is what releases
         * them — an orphan-only drop would hang the scope's owner. */
        scope_done(c);
    }
}

/* defer stack — per task so entries survive suspends (a parked frame
 * must not leak its defers onto the next task sharing this thread);
 * TLS only for foreign threads with no task */
static _Thread_local RtDefer *tls_defers;

static RtDefer **defer_head(void) {
    return tls_task ? &tls_task->defers : &tls_defers;
}

RtDefer *rt_defer_mark(void) { return *defer_head(); }

void rt_defer_push(void (*fp)(void *), void *env) {
    RtDefer *d = xmalloc(sizeof *d);
    d->fp = fp;
    d->env = env;
    RtDefer **hp = defer_head();
    d->next = *hp;
    *hp = d;
}

void rt_defer_drain_to(RtDefer *mark) {
    RtDefer **hp = defer_head();
    while (*hp != mark) {
        RtDefer *d = *hp;
        *hp = d->next;
        d->fp(d->env);
        free(d->env);
        free(d);
    }
}

static void task_defers_drain(RtTask *t) {
    while (t->defers) {
        RtDefer *d = t->defers;
        t->defers = d->next;
        d->fp(d->env);
        free(d->env);
        free(d);
    }
}

/* cancel/unwind for thread tasks: oxl_thread_exit would skip the settle
 * (scope_done + free) AND kill the pool worker — unwind into the
 * task's own setjmp slot instead, exactly like frame traps */
static void task_unwind(void) {
    RtTask *t = tls_task;
    if (!t) oxl_thread_exit(0); /* foreign thread: nothing to settle */
    park_del(t);
    longjmp(tls_jb, 1);
}

/* settle a finished task's frame: inner scopes are fields of the
 * frame, so a task that dies mid-block must not free() while children
 * still point at them. rt_trap already orphan-marks the trap-time
 * scope chain; the clean/RT_CANCEL exit path walks fr->cur_scope (the
 * last-suspend innermost) and marks whatever was abandoned — live
 * scopes keep a frame hold that their last child drops, dead ones are
 * torn down inline. The self-hold makes the frees last-dropper-wins:
 * whoever drops holds to 0 frees the frame. */
static void task_release_frame(RtTask *t, int trapped) {
    /* a trapped frame task's inner scopes were orphan-marked inside
     * rt_trap while its stack was live — everyone else walks
     * fr->cur_scope (the last-suspend innermost) here. Live orphans
     * keep a frame hold their last child drops; drained ones tear
     * down inline. The spawn-time run ref keeps the frame allocated
     * through the walk — the final unref is the last drop. */
    if (t->is_frame && !trapped) {
        for (RtScope *x = t->frame->cur_scope;
             x && x != t->scope;) {
            oxl_mu_lock(&x->mu);
            x->orphan = 1;
            x->cancel = 1;
            int live = x->pending > 0;
            /* live orphans take a temp pending ref so x->mu and the
             * parked list stay alive through the wake pass — without it
             * the last child's scope_done can tear x down while
             * scope_cancel_wake still walks it (rt_trap protocol) */
            if (live) { task_ref(t); x->pending++; }
            else x->dead = 1; /* tearing down below — traps must skip */
            RtScope *up = x->parent;
            oxl_mu_unlock(&x->mu);
            if (!live) { scope_teardown(x); x = up; continue; }
            scope_cancel_wake(x);
            /* scope_done: same pending->0 drain for these orphans */
            scope_done(x);
            x = up;
        }
    }
    task_unref(t);
}

static void run_task(RtTask *t) {
    tls_scope = t->scope;
    tls_task = t;
    volatile int trapped = 0;
    if (t->is_frame) {
        t->wstate = WS_ONSTACK;
        if (setjmp(tls_jb) == 0) {
            if (t->poll(t->frame, t) == RT_PENDING) {
                /* leaving the C stack: a waker that already fired
                 * planted WAKE_REQ — self-enqueue instead of parking */
                if (__sync_lock_test_and_set(&t->wstate, WS_PARKED) == WS_WAKEREQ) {
                    tls_task = NULL;
                    pool_push(t);
                } else {
                    tls_task = NULL;
                }
                return;
            }
        } else {
            trapped = 1;
        }
        /* READY / CANCEL / trapped(longjmp) all land here */
    } else {
        if (setjmp(tls_jb) == 0) t->fn(t->arg);
        else trapped = 1;
        /* cancel-unwound (task_unwind) lands here */
    }
    /* defers of unwound fns still run — the task dies either way */
    /* a trap/unwind may have left tls_task NULL and tls_scope on a
     * torn-down inner scope: run defers in the task's spawn scope */
    tls_task = t;
    tls_scope = t->scope;
    task_defers_drain(t);
    tls_task = NULL;
    if (t->sel) rt_select_arm(t); /* dropped select wait: reclaim it */
    scope_done(t->scope);
    task_release_frame(t, trapped);
}

/* oxl_thread_spawn refused: spawn must never silently drop caller-owned
 * state packed into the arg frame (fds, slots — their cleanup only
 * exists inside the fn). Drive the fn task on the calling thread
 * instead: run_task does the full settle (defers, scope_done, run-ref
 * drop). TLS and the jump buffer are saved first — run_task installs
 * its own setjmp target and leaves tls_* pointing at the dead task. */
static void run_inline(RtTask *t) {
    RtTask *pt = tls_task;
    RtScope *ps = tls_scope;
    jmp_buf pj;
    memcpy(pj, tls_jb, sizeof pj);
    run_task(t);
    memcpy(tls_jb, pj, sizeof pj);
    tls_task = pt;
    tls_scope = ps;
}

/* oxl_thread_spawn refused. First fallback: hand the task to the work
 * pool — the async contract holds (a blocking task must NOT run on the
 * caller: an HTTP accept loop would stall for the whole request), and
 * settle still does the full cleanup. Only when the pool has no live
 * worker at all do we drive the task on this thread — dropping
 * caller-owned state is never acceptable, and with zero threads an
 * inline run is the only execution left. */
static void spawn_fallback(RtTask *t) {
    t->external = 0; /* a pooled task wakes via pool_push, not a local wk */
    t->wk = NULL;
    pool_start();
    oxl_mu_lock(&pool.mu);
    int live = pool.workers > 0;
    oxl_mu_unlock(&pool.mu);
    if (live) { pool_push(t); return; }
    if (t->is_frame) {
        RtScope *sc = t->scope; /* run_root drops the run ref — pin sc */
        rt_run_root((RtFrame *)(t + 1));
        scope_done(sc);
    } else {
        run_inline(t);
    }
}

static void *worker(void *_p) {
    (void)_p;
    for (;;) {
        oxl_mu_lock(&pool.mu);
        while (!pool.head) oxl_cv_wait(&pool.cv, &pool.mu);
        RtTask *t = pool.head;
        pool.head = t->next;
        if (!pool.head) pool.tail = NULL;
        oxl_mu_unlock(&pool.mu);
        if (!t->scope || !t->scope->cancel) run_task(t);
        else {
            /* cancelled before it ran: earlier poll rounds may still
             * have registered defers — drain them under a live setjmp
             * so a suspending defer unwinds back here (like the trap
             * path) instead of into dead jump state, then settle
             * exactly like run_task */
            tls_scope = t->scope;
            tls_task = t;
            setjmp(tls_jb);
            task_defers_drain(t);
            tls_task = NULL;
            if (t->sel) rt_select_arm(t);
            scope_done(t->scope);
            /* suspended before cancel: inner scopes may sit open in
             * the frame — orphan them so their children settle and the
             * frame frees at the last drain, same as the trap path */
            task_release_frame(t, 0);
        }
    }
    return 0;
}

static void pool_start(void) {
    oxl_mu_lock(&pool.mu);
    /* fill once, retry while the pool is dead: a transient create
     * failure at startup must not pin the pool at zero forever */
    if (!pool.started || pool.workers == 0) {
#ifdef _WIN32
        SYSTEM_INFO si;
        GetSystemInfo(&si);
        long n = (long)si.dwNumberOfProcessors;
#else
        long n = sysconf(_SC_NPROCESSORS_ONLN);
#endif
        if (n < 2) n = 2;
        if (n > 32) n = 32;
        for (long i = pool.workers; i < n; i++) {
            oxl_thread_t t;
            if (oxl_thread_spawn(&t, NULL, worker, NULL) != 0) break;
            oxl_thread_detach(t);
            pool.workers++;
        }
        pool.started = 1;
    }
    oxl_mu_unlock(&pool.mu);
}

static void pool_push(RtTask *t) {
    pool_start();
    t->next = NULL;
    oxl_mu_lock(&pool.mu);
    if (pool.tail) pool.tail->next = t; else pool.head = t;
    pool.tail = t;
    oxl_cv_signal(&pool.cv);
    oxl_mu_unlock(&pool.mu);
}

/* re-enqueue a suspended frame task: external drivers wait on their own
 * cond; pool tasks go back on the shared queue. The swap serializes
 * against the driver's just-leaving-stack window. */
void rt_task_wake(RtTask *t) {
    if (t->external) {
        if (__sync_lock_test_and_set(&t->wstate, WS_WAKEREQ) == WS_PARKED) {
            RtWake *w = t->wk;
            oxl_mu_lock(&w->mu);
            w->ready = 1;
            oxl_cv_signal(&w->cv);
            oxl_mu_unlock(&w->mu);
        }
    } else {
        if (__sync_lock_test_and_set(&t->wstate, WS_WAKEREQ) == WS_PARKED)
            pool_push(t);
    }
}

static void *task_wrap(void *p) {
    RtTask *t = p;
    run_task(t);
    return 0;
}

/* blocking-pool frame task: a dedicated thread drives the state machine
 * to completion (suspend points park this thread on the task's wake cv) */
static void *task_wrap_frame(void *p) {
    RtTask *t = p;
    RtWake wk;
    oxl_mu_init(&wk.mu, NULL);
    oxl_cv_init(&wk.cv, NULL);
    wk.ready = 0;
    t->wk = &wk;
    tls_scope = t->scope;
    tls_task = t;
    volatile int trapped = 0;
    if (setjmp(tls_jb) == 0) {
        while (1) {
            /* cancelled before it ran: settle like the pool worker
             * instead of re-polling into a dead scope */
            if (t->scope && t->scope->cancel) break;
            t->wstate = WS_ONSTACK;
            if (t->poll(t->frame, t) != RT_PENDING) break;
            if (__sync_lock_test_and_set(&t->wstate, WS_PARKED) == WS_WAKEREQ)
                continue; /* wake already landed */
            oxl_mu_lock(&wk.mu);
            while (!wk.ready) oxl_cv_wait(&wk.cv, &wk.mu);
            wk.ready = 0;
            oxl_mu_unlock(&wk.mu);
        }
    } else {
        trapped = 1;
    }
    task_defers_drain(t);
    tls_task = NULL;
    if (t->sel) rt_select_arm(t);
    scope_done(t->scope);
    t->wk = NULL;
    oxl_mu_destroy(&wk.mu);
    oxl_cv_destroy(&wk.cv);
    task_release_frame(t, trapped);
    return 0;
}

static void scope_pending_up(RtScope *s) {
    if (!s) return;
    oxl_mu_lock(&s->mu);
    s->pending++;
    oxl_mu_unlock(&s->mu);
}

void rt_spawn(void *(*fn)(void *), void *arg) {
    RtScope *s = tls_scope;
    scope_pending_up(s);
    if (!s) { /* outside scope: dedicated thread, nothing to join */
        RtTask *t = xmalloc(sizeof *t);
        memset(t, 0, sizeof *t);
        task_ref(t); /* run ref — dropped at settle */
        t->fn = fn; t->arg = arg;
        oxl_thread_t th;
        if (oxl_thread_spawn(&th, NULL, task_wrap, t) == 0) oxl_thread_detach(th);
        else spawn_fallback(t); /* never drop the task — pool or inline */
        return;
    }
    RtTask *t = xmalloc(sizeof *t);
    memset(t, 0, sizeof *t);
    task_ref(t); /* run ref — dropped at settle */
    t->fn = fn; t->arg = arg; t->scope = s;
    pool_push(t);
}

/* blocking pool: tasks that may block in FFI run on dedicated threads --
 * a blocked worker can never starve the work pool */
void rt_spawnb(void *(*fn)(void *), void *arg) {
    RtScope *s = tls_scope;
    scope_pending_up(s);
    RtTask *t = xmalloc(sizeof *t);
    memset(t, 0, sizeof *t);
    task_ref(t); /* run ref — dropped at settle */
    t->fn = fn; t->arg = arg; t->scope = s;
    oxl_thread_t th;
    if (oxl_thread_spawn(&th, NULL, task_wrap, t) == 0) oxl_thread_detach(th);
    else spawn_fallback(t); /* settle does scope_done + the run ref */
}

/* one allocation per task: [RtTask][frame bytes]. Generated code gets
 * the frame pointer and fills param slots before rt_spawn_task /
 * rt_spawnb_task / rt_run_root take the task live. */
RtFrame *rt_task_new(RtPoll poll, int64_t bytes) {
    RtTask *t = xmalloc(sizeof(RtTask) + (size_t)bytes);
    memset(t, 0, sizeof(RtTask) + (size_t)bytes);
    t->poll = poll;
    t->is_frame = 1;
    task_ref(t); /* run ref — dropped at settle */
    t->scope = tls_scope;
    RtFrame *fr = (RtFrame *)(t + 1);
    fr->cur_scope = tls_scope;
    t->frame = fr;
    return fr;
}

static RtTask *task_of(RtFrame *fr) { return ((RtTask *)fr) - 1; }

void rt_spawn_task(RtFrame *fr) {
    RtTask *t = task_of(fr);
    scope_pending_up(t->scope);
    pool_push(t);
}

void rt_spawnb_task(RtFrame *fr) {
    RtTask *t = task_of(fr);
    scope_pending_up(t->scope);
    t->external = 1;
    oxl_thread_t th;
    if (oxl_thread_spawn(&th, NULL, task_wrap_frame, t) == 0) oxl_thread_detach(th);
    else spawn_fallback(t); /* pool keeps the pending slot + frame alive */
}

/* drive one frame task to completion on the calling thread — the root
 * task (oxl_main) and test-harness tasks use this; parked tasks signal
 * wk via rt_task_wake */
int rt_run_root(RtFrame *fr) {
    RtTask *t = task_of(fr);
    RtWake wk;
    oxl_mu_init(&wk.mu, NULL);
    oxl_cv_init(&wk.cv, NULL);
    wk.ready = 0;
    t->external = 1;
    t->wk = &wk;
    RtTask *prev = tls_task;
    RtScope *prev_scope = tls_scope; /* a trapping frame task unwinds past
        every scope_leave — the caller's scope must come back or later
        rt_scope_enter/rt_alloc on this thread walk freed scope memory */
    jmp_buf prev_jb; /* same for the caller's setjmp target: a nested
        driver (spawn-failure fallback) must not leave traps landing
        in this frame's dead stack */
    memcpy(prev_jb, tls_jb, sizeof prev_jb);
    tls_task = t;
    tls_scope = t->scope;
    volatile int trapped = 0;
    if (setjmp(tls_jb) == 0) {
        while (1) {
            /* same cancel gate as the pool worker: a cancel-woken task
             * settles instead of re-polling into a dead scope */
            if (t->scope && t->scope->cancel) break;
            t->wstate = WS_ONSTACK;
            if (t->poll(fr, t) != RT_PENDING) break;
            if (__sync_lock_test_and_set(&t->wstate, WS_PARKED) == WS_WAKEREQ)
                continue;
            oxl_mu_lock(&wk.mu);
            while (!wk.ready) oxl_cv_wait(&wk.cv, &wk.mu);
            wk.ready = 0;
            oxl_mu_unlock(&wk.mu);
        }
    } else {
        trapped = 1;
    }
    task_defers_drain(t);
    memcpy(tls_jb, prev_jb, sizeof prev_jb);
    tls_task = prev;
    tls_scope = prev_scope;
    if (t->sel) rt_select_arm(t);
    t->wk = NULL;
    oxl_mu_destroy(&wk.mu);
    oxl_cv_destroy(&wk.cv);
    task_release_frame(t, trapped);
    return trapped;
}

/* cooperative cancel check -- tasks observe at suspend points */
int rt_cancelled(void) { return tls_scope && tls_scope->cancel; }

int64_t rt_task_data(RtTask *t) { return t->chan_res; }

static void scope_heap_free(RtScope *s) {
    ObjHdr *h = s->heap;
    while (h) {
        ObjHdr *n = h->hnext;
        free(h);
        h = n;
    }
    s->heap = NULL;
}

static int join_scope(RtScope *s) {
    oxl_mu_lock(&s->mu);
    while (s->pending > 0) oxl_cv_wait(&s->cv, &s->mu);
    int err = s->trapped ? (int)s->err : 0;
    s->dead = 1; /* teardown commits here: trap walks must skip us */
    oxl_mu_unlock(&s->mu);
    RtScope *up = s->parent; /* captured pre-detach for tls_scope restore */
    scope_unlink(s);
    scope_detach_children(s);
    oxl_mu_destroy(&s->mu);
    oxl_cv_destroy(&s->cv);
    scope_heap_free(s);
    free(s->arena_buf); /* bulk-free: the arena story */
    tls_scope = up;
    return err;
}

/* frame-task join: register as the scope's joiner, or READY if drained */
int rt_scope_join_poll(RtScope *s, RtTask *t) {
    park_add(t, t->scope, s, WK_JOIN);
    oxl_mu_lock(&s->mu);
    if (s->pending > 0) {
        s->join_task = t;
        oxl_mu_unlock(&s->mu);
        return RT_PENDING;
    }
    oxl_mu_unlock(&s->mu);
    if (park_claim(t)) park_del(t); /* self-unpark on the no-wait path */
    return RT_READY;
}

int rt_scope_finish(RtScope *s) {
    oxl_mu_lock(&s->mu);
    int err = s->trapped ? (int)s->err : 0;
    s->dead = 1;
    oxl_mu_unlock(&s->mu);
    RtScope *up = s->parent;
    scope_unlink(s);
    scope_detach_children(s);
    oxl_mu_destroy(&s->mu);
    oxl_cv_destroy(&s->cv);
    scope_heap_free(s);
    free(s->arena_buf);
    tls_scope = up;
    return err;
}

/* propagate to the PARENT: the scope that just left is already
 * unlinked (its parent field is NULL), so callers pass the scope
 * join_scope/rt_scope_finish restored to tls_scope */
void rt_scope_propagate(RtScope *p, int64_t err) {
    if (p) {
        oxl_mu_lock(&p->mu);
        p->trapped = 1;
        p->err = err; /* keep the code — propagation must not swallow it */
        oxl_mu_unlock(&p->mu);
    } else {
        fprintf(stderr, "trap %lld escaped top scope\n", (long long)err);
        abort();
    }
}

void rt_scope_leave(RtScope *s) {
    int err = join_scope(s);
    if (err) rt_scope_propagate(tls_scope, err);
}

int rt_scope_try(RtScope *s) { return join_scope(s); }

void rt_trap(int64_t code) {
    RtScope *s = tls_scope;
    if (!s) { fprintf(stderr, "trap %lld outside scope\n", (long long)code); abort(); }
    RtTask *t = tls_task;
    if (t && t->is_frame) {
        /* the spawn-time run ref already pins the frame: orphan drains
         * can never drop holds to 0 until this task settles, so no
         * separate self-hold is needed here */
        /* mark the whole chain from innermost to the task's spawn scope —
         * inner scopes live inside this task's frame and never join, so
         * orphan each: live ones keep a frame hold their last child
         * drops; already-drained ones tear down now, here on the live
         * stack, since no joiner is coming */
        for (RtScope *x = s; x; x = x->parent) {
            oxl_mu_lock(&x->mu);
            if (!x->trapped) { x->trapped = 1; x->err = code; }
            x->cancel = 1;
            if (x != t->scope) {
                /* orphan + a temp pending ref: the scope's mutex and
                 * parked list must stay alive through the whole wake
                 * pass — teardown waits for the temp drop below. One
                 * frame hold per orphan; whoever drains the scope (the
                 * last child's scope_done, or the temp drop itself)
                 * releases it via scope_orphan_free. */
                x->orphan = 1;
                x->pending++;
                task_ref(t); /* orphan hold — scope_orphan_free drops it */
            }
            oxl_mu_unlock(&x->mu);
            if (x == t->scope) break;
        }
        /* wake parked tasks so they observe cancel at resume instead
         * of parking forever on a chan nobody will serve */
        for (RtScope *x = s; x; x = x->parent) {
            scope_cancel_wake(x);
            if (x == t->scope) break;
        }
        /* descendants of every marked scope: tasks parked inside
         * sibling subtrees aren't on this task's ancestor chain —
         * without cancelling them they park forever and their pending
         * slots keep the join open */
        oxl_mu_lock(&cancel_tree_mu);
        for (RtScope *x = s; x; x = x->parent) {
            scope_cancel_tree(x);
            if (x == t->scope) break;
        }
        oxl_mu_unlock(&cancel_tree_mu);
        /* drop the temp refs: a drained orphan tears down here (its
         * owner hold drops with it); live ones drain later under their
         * last child's scope_done */
        for (RtScope *x = s; x; ) {
            /* capture pre-drop: scope_done can drain x, and teardown
             * NULLs x->parent — reading it after would end the walk
             * early and leak every ancestor's temp hold */
            RtScope *up = x->parent;
            if (x != t->scope) scope_done(x); /* chain orphan: drains on 0 */
            if (x == t->scope) break;
            x = up;
        }
        tls_task = NULL;
        longjmp(tls_jb, 1); /* run_task settles the pending slot */
    }
    if (!t) {
        /* trap on a non-task thread (main/foreign) — no joiner exists to
         * consume it; defers on the thread stack still unwind, then fail
         * loudly instead of oxl_thread_exit's silent exit-0 */
        while (tls_defers) {
            RtDefer *d = tls_defers;
            tls_defers = d->next;
            d->fp(d->env);
            free(d->env);
            free(d);
        }
        fprintf(stderr, "trap %lld\n", (long long)code);
        fflush(NULL);
        abort();
    }
    /* thread task: same chain walk as the frame path — mark every scope
     * from innermost up to the task's spawn scope so the trap reaches
     * t->scope (its joiner reads `trapped`/`err`) and siblings cancel,
     * then wake each level's parked tasks. Inner scopes live on THIS
     * thread's C stack: drained ones tear down inline while the stack
     * is still valid. A live orphan can't be handed to children — the
     * scope struct unwinds with us — so it fails loudly instead of
     * dangling. The checker bans `scope {}` in non-susp fns, so past
     * t->scope the chain is empty in practice. */
    for (RtScope *x = s; x; x = x->parent) {
        oxl_mu_lock(&x->mu);
        if (!x->trapped) { x->trapped = 1; x->err = code; }
        x->cancel = 1;
        if (x != t->scope) { x->orphan = 1; x->pending++; }
        oxl_mu_unlock(&x->mu);
        if (x == t->scope) break;
    }
    for (RtScope *x = s; x; x = x->parent) {
        scope_cancel_wake(x);
        if (x == t->scope) break;
    }
    /* descendant cancel, same as the frame path — nested scopes below
     * the chain belong to other tasks' frames (heap), safe to walk */
    oxl_mu_lock(&cancel_tree_mu);
    for (RtScope *x = s; x; x = x->parent) {
        scope_cancel_tree(x);
        if (x == t->scope) break;
    }
    oxl_mu_unlock(&cancel_tree_mu);
    for (RtScope *x = s; x; ) {
        /* capture pre-drop: teardown NULLs x->parent (see frame path) */
        RtScope *up = x->parent;
        if (x != t->scope) {
            int drain = 0;
            oxl_mu_lock(&x->mu);
            if (--x->pending <= 0) { x->dead = 1; drain = 1; }
            oxl_mu_unlock(&x->mu);
            if (drain) scope_teardown(x); /* stack scope, still live */
            else {
                fprintf(stderr, "trap: thread task abandoned live stack scope\n");
                abort();
            }
        }
        if (x == t->scope) break;
        x = up;
    }
    task_unwind(); /* run_task drains defers + settles pending */
}

/* ---------------- Shared<T>: atomic word cell ---------------- */

RtShared *rt_mkshared(int64_t v) {
    RtShared *s = xmalloc(sizeof *s);
    s->val = v;
    s->writer = 0;
    return s;
}

int64_t rt_sget(RtShared *s) {
    return __sync_fetch_and_add(&s->val, 0); /* atomic read */
}

void rt_sset(RtShared *s, int64_t v) {
    /* seq-cst store: munlock must publish the critical section's writes —
       lock_test_and_set is acquire-only, so on weak targets a trailing
       fence can't stop earlier stores sinking past the unlock */
    __atomic_store_n(&s->val, v, __ATOMIC_SEQ_CST);
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
#ifdef _WIN32
    /* FILETIME: 100ns ticks since 1601; PreciseAsFileTime tracks the
     * system clock at sub-ms resolution (plain AsFileTime quantizes) */
    FILETIME ft;
    GetSystemTimePreciseAsFileTime(&ft);
    uint64_t t = ((uint64_t)ft.dwHighDateTime << 32) | ft.dwLowDateTime;
    return (int64_t)((t - UINT64_C(116444736000000000)) / 10000);
#else
    struct timespec ts;
    clock_gettime(CLOCK_REALTIME, &ts);
    return (int64_t)ts.tv_sec * 1000 + ts.tv_nsec / 1000000;
#endif
}
/* usleep(3) is obsolescent (gone in POSIX.1-2008) and fails EINVAL past
 * 1e6us — silent no-sleep; useconds_t truncation corrupts longer waits. */
static void sleep_us(int64_t us) {
    if (us <= 0)
        return;
#ifdef _WIN32
    Sleep((DWORD)((us + 999) / 1000));
#else
    struct timespec ts = {us / 1000000, (us % 1000000) * 1000};
    while (nanosleep(&ts, &ts) != 0 && errno == EINTR) {
    }
#endif
}
void rt_usleep(int64_t usecs) { sleep_us(usecs); }

/* ---------------- channels (unbuffered, word payload) ----------------
 * One chan, two surfaces: thread tasks block on cond vars, frame tasks
 * park on wait queues and get re-enqueued by rt_task_wake. Rendezvous
 * rule is unchanged: a send completes only once a receiver takes it. */

typedef struct SendReq {
    RtTask *t;
    int64_t v;
    struct SendReq *next;
} SendReq;

/* select waiters: one node per arm chan, all owned by the RtSel */
typedef struct SelW {
    struct RtSel *sel;
    RtChan *ch;            /* back-link for later unlink */
    struct SelW *next;     /* same-chan selq link */
    int slot;
} SelW;

struct RtSel {
    volatile int winner;   /* slot index once claimed; -1 while parked */
    volatile int refs;     /* task + (optional) timer + drain refs */
    int n;
    int64_t tmo;           /* ms; timer armed when >= 0 */
    RtTask *t;
    SelW w[];
};

/* the RtSel pins its task: while any RtSel ref is held (task, timer,
 * or a drain that detached SelW nodes) s->t cannot be freed — the ref
 * taken at arm time is released only when the RtSel itself dies */
static void sel_unref(struct RtSel *s) {
    if (__sync_sub_and_fetch(&s->refs, 1) == 0) {
        RtTask *ot = s->t;
        free(s);
        task_unref(ot);
    }
}

struct RtChan {
    oxl_mutex_t mu;
    oxl_cond_t cv_send, cv_recv;
    int64_t *buf;       /* ring storage — NULL when cap==0 */
    int64_t cap;        /* buffered capacity (mkchan(n)) */
    int64_t head;       /* ring pop index; tail = (head+count)%cap */
    int64_t count;      /* ring entries */
    int64_t val;
    int has, closed;
    RtTask *slot_owner; /* thread sender that deposited `val` */
    int trecv;          /* thread receivers inside cv_recv wait */
    RtTask *recvq;      /* parked frame receivers (task->next chain) */
    SendReq *sendq;     /* parked frame sends */
    SelW *selq;         /* parked select waiters */
};

static SendReq *chan_pop_send(RtChan *ch); /* defined below */

/* under ch->mu, count>0: pop the ring head, refill the slot from a
 * parked frame sender (FIFO + the sender's wake), deliver v. */
static int64_t chan_ring_pop(RtChan *ch, RtTask **wake) {
    int64_t v = ch->buf[ch->head];
    ch->head = (ch->head + 1) % ch->cap;
    ch->count--;
    SendReq *s = chan_pop_send(ch);
    if (s) {
        ch->buf[(ch->head + ch->count) % ch->cap] = s->v;
        ch->count++;
        *wake = s->t;
        free(s);
    }
    return v;
}

/* under ch->mu: hand `v` to the first live select waiter. returns 1 when
 * a waiter claimed the fire (ch->mu released + task woken). */
static int sel_fire(RtChan *ch, int64_t v) {
    while (ch->selq) {
        SelW *w = ch->selq;
        ch->selq = w->next;
        struct RtSel *s = w->sel;
        /* the popped node's task ref is ours now: s->t stays valid
         * memory win or lose, then the ref leaves with the node */
        if (!park_claim(s->t)) { task_unref(s->t); continue; }
        s->winner = w->slot;
        s->t->chan_res = v;
        s->t->sel_arm = w->slot;
        RtTask *t = s->t;
        oxl_mu_unlock(&ch->mu);
        rt_task_wake(t);
        task_unref(t);
        return 1;
    }
    return 0;
}

/* park a frame receiver: node = the task itself on recvq */
static void chan_park_recv(RtChan *ch, RtTask *t) {
    park_add(t, t->scope, ch, WK_RECV);
    task_ref(t); /* the recvq node's ref — leaves with the node */
    t->next = ch->recvq;
    ch->recvq = t;
}

/* pop the first claimable parked receiver. ch->mu held; on success the
 * claim is ours (ch->mu still held — caller fills + unlocks + wakes). */
static RtTask *chan_pop_recv(RtChan *ch) {
    while (ch->recvq) {
        RtTask *r = ch->recvq;
        ch->recvq = r->next;
        if (!park_claim(r)) { task_unref(r); continue; } /* dead node */
        park_del(r);
        return r; /* node ref rides out — caller drops it after the wake */
    }
    return NULL;
}

/* pop the first claimable parked sender. Same contract: caller takes the
 * value and wakes the task after unlock. */
static SendReq *chan_pop_send(RtChan *ch) {
    while (ch->sendq) {
        SendReq *r = ch->sendq;
        ch->sendq = r->next;
        if (!park_claim(r->t)) { task_unref(r->t); free(r); continue; }
        park_del(r->t);
        return r; /* r->t's node ref rides out — caller drops post-wake */
    }
    return NULL;
}

RtChan *rt_mkchan(int64_t cap) {
    RtChan *c = xcalloc(1, sizeof *c);
    oxl_mu_init(&c->mu, NULL);
    oxl_cv_init(&c->cv_send, NULL);
    oxl_cv_init(&c->cv_recv, NULL);
    c->cap = cap < 0 ? 0 : cap;
    if (c->cap > 0) c->buf = xcalloc((size_t)c->cap, sizeof(int64_t));
    return c;
}

void rt_send(RtChan *ch, int64_t v) {
    if (rt_cancelled()) task_unwind();
    RtTask *me = tls_task;
    oxl_mu_lock(&ch->mu);
    park_add(me, me ? me->scope : NULL, ch, WK_TSEND);
    for (;;) {
        /* cancelled() checked under ch->mu: the cancel pass sets the flag
         * before claiming the park entry and broadcasting, so a wake that
         * landed before we locked still shows the flag here — without the
         * under-lock test that cancel is lost and we cond_wait forever */
        while (ch->has && !ch->closed && !rt_cancelled())
            oxl_cv_wait(&ch->cv_send, &ch->mu);
        if (ch->closed) break;
        if (rt_cancelled()) goto cancelled; /* cancel-wake must not re-wait */
        RtTask *r = chan_pop_recv(ch);
        if (r) { /* hand directly to a parked frame receiver */
            r->chan_res = v;
            oxl_mu_unlock(&ch->mu);
            rt_task_wake(r);
            task_unref(r); /* the recvq node ref we inherited */
            goto done;
        }
        if (sel_fire(ch, v)) goto done; /* sel_fire releases ch->mu */
        if (ch->count < ch->cap) { /* buffered: drop in the ring */
            ch->buf[(ch->head + ch->count) % ch->cap] = v;
            ch->count++;
            oxl_cv_broadcast(&ch->cv_recv);
            oxl_mu_unlock(&ch->mu);
            goto done;
        }
        ch->val = v;
        ch->has = 1;
        ch->slot_owner = me;
        oxl_cv_broadcast(&ch->cv_recv);
        while (ch->has && !ch->closed && !rt_cancelled())
            oxl_cv_wait(&ch->cv_send, &ch->mu);
        if (rt_cancelled()) {
            /* retract our own unclaimed slot only — a 1 here could be
             * another thread sender's fresh deposit */
            if (ch->has && ch->slot_owner == me) { ch->has = 0; ch->val = 0; ch->slot_owner = NULL; }
            goto cancelled;
        }
        /* consumed or closed mid-wait: the send is complete either way
         * under the v0 send-on-closed rule */
        break;
    }
    oxl_mu_unlock(&ch->mu);
done:
    if (me && park_claim(me)) park_del(me);
    return;
cancelled:
    oxl_mu_unlock(&ch->mu);
    if (me && park_claim(me)) park_del(me);
    task_unwind();
}

int64_t rt_recv(RtChan *ch) {
    if (rt_cancelled()) task_unwind();
    RtTask *me = tls_task;
    int64_t v = 0;
    park_add(me, me ? me->scope : NULL, ch, WK_TRECV);
    for (;;) {
        oxl_mu_lock(&ch->mu);
        if (ch->count > 0) { /* buffered: drain oldest, refill from sendq */
            RtTask *w = NULL;
            v = chan_ring_pop(ch, &w);
            oxl_mu_unlock(&ch->mu);
            if (w) { rt_task_wake(w); task_unref(w); }
            goto done;
        }
        SendReq *r = chan_pop_send(ch); /* parked frame sender delivers */
        if (r) {
            v = r->v;
            RtTask *st = r->t;
            free(r);
            oxl_mu_unlock(&ch->mu);
            rt_task_wake(st);
            task_unref(st); /* the sendq node ref we inherited */
            goto done;
        }
        if (ch->has || ch->closed) break;
        /* same under-lock cancel test as rt_send — a claim+broadcast
         * that landed before the lock left the flag set, not a wake */
        if (rt_cancelled()) {
            oxl_mu_unlock(&ch->mu);
            if (me && park_claim(me)) park_del(me);
            task_unwind();
        }
        ch->trecv++;
        oxl_cv_wait(&ch->cv_recv, &ch->mu);
        ch->trecv--;
        oxl_mu_unlock(&ch->mu);
        if (rt_cancelled()) {
            if (me && park_claim(me)) park_del(me);
            task_unwind();
        }
    }
    v = ch->has ? ch->val : 0; /* closed+empty delivers 0, not a stale slot */
    ch->has = 0;
    ch->val = 0;
    /* broadcast, not signal: with racing senders a lone wakeup can land
     * on the *completing* send instead of the queued one and strand it */
    oxl_cv_broadcast(&ch->cv_send);
    oxl_mu_unlock(&ch->mu);
done:
    if (me && park_claim(me)) park_del(me);
    return v;
}

/* frame-task recv: READY delivers via task->chan_res (uniform for the
 * immediate and the woken paths); PENDING parks on recvq */
int rt_recv_poll(RtChan *ch, RtTask *t) {
    oxl_mu_lock(&ch->mu);
    if (ch->count > 0) { /* buffered: drain oldest, refill from sendq */
        RtTask *w = NULL;
        t->chan_res = chan_ring_pop(ch, &w);
        oxl_mu_unlock(&ch->mu);
        if (w) { rt_task_wake(w); task_unref(w); }
        return RT_READY;
    }
    SendReq *r = chan_pop_send(ch);
    if (r) {
        t->chan_res = r->v;
        RtTask *st = r->t;
        free(r);
        oxl_mu_unlock(&ch->mu);
        rt_task_wake(st);
        task_unref(st);
        return RT_READY;
    }
    if (ch->has || ch->closed) {
        t->chan_res = ch->has ? ch->val : 0;
        ch->has = 0;
        ch->val = 0;
        oxl_cv_broadcast(&ch->cv_send);
        oxl_mu_unlock(&ch->mu);
        return RT_READY;
    }
    chan_park_recv(ch, t);
    oxl_mu_unlock(&ch->mu);
    return RT_PENDING;
}

int rt_send_poll(RtChan *ch, int64_t v, RtTask *t) {
    oxl_mu_lock(&ch->mu);
    RtTask *r = chan_pop_recv(ch);
    if (r) {
        r->chan_res = v;
        oxl_mu_unlock(&ch->mu);
        rt_task_wake(r);
        task_unref(r);
        return RT_READY;
    }
    if (sel_fire(ch, v)) return RT_READY; /* sel_fire releases ch->mu */
    if (ch->count < ch->cap) { /* buffered: drop in the ring */
        ch->buf[(ch->head + ch->count) % ch->cap] = v;
        ch->count++;
        oxl_cv_broadcast(&ch->cv_recv);
        oxl_mu_unlock(&ch->mu);
        return RT_READY;
    }
    if (ch->closed) {
        oxl_mu_unlock(&ch->mu);
        return RT_READY; /* send on closed completes silently (v0 rule) */
    }
    /* a thread receiver is committed inside cv_wait — but only plant
     * the slot when it's empty: a `has` already set is another thread
     * sender's deposit that receiver hasn't consumed yet; overwriting
     * it would lose that value while both senders report success */
    if (ch->trecv > 0 && !ch->has) {
        ch->val = v;
        ch->has = 1;
        ch->slot_owner = NULL; /* frame deposit: no thread owner */
        oxl_cv_broadcast(&ch->cv_recv);
        oxl_mu_unlock(&ch->mu);
        return RT_READY;
    }
    SendReq *sr = xmalloc(sizeof *sr);
    sr->t = t;
    sr->v = v;
    sr->next = ch->sendq;
    ch->sendq = sr;
    task_ref(t); /* the sendq node's ref — leaves with the node */
    park_add(t, t->scope, ch, WK_SEND);
    oxl_mu_unlock(&ch->mu);
    return RT_PENDING;
}

void rt_close(RtChan *ch) {
    oxl_mu_lock(&ch->mu);
    ch->closed = 1;
    oxl_cv_broadcast(&ch->cv_send);
    oxl_cv_broadcast(&ch->cv_recv);
    /* drain parked frame tasks: receivers complete with the slot value
     * (matching the thread path); senders complete silently. Each wake
     * still races the parked claim — dead nodes are skipped. */
    RtTask *r = ch->recvq;
    ch->recvq = NULL;
    SendReq *sq = ch->sendq;
    ch->sendq = NULL;
    SelW *sw = ch->selq;
    ch->selq = NULL;
    /* pin each RtSel before unlocking: the woken task's rt_select_arm
     * can unref+free it while we still walk detached nodes */
    for (SelW *w = sw; w; w = w->next)
        __sync_add_and_fetch(&w->sel->refs, 1);
    int64_t val = ch->has ? ch->val : 0;
    ch->has = 0;
    ch->val = 0;
    oxl_mu_unlock(&ch->mu);
    while (r) {
        RtTask *n = r->next;
        if (park_claim(r)) {
            park_del(r);
            r->chan_res = val;
            rt_task_wake(r);
        }
        task_unref(r); /* recvq node ref */
        r = n;
    }
    while (sq) {
        SendReq *n = sq->next;
        if (park_claim(sq->t)) {
            park_del(sq->t);
            rt_task_wake(sq->t);
        }
        task_unref(sq->t); /* sendq node ref */
        free(sq);
        sq = n;
    }
    while (sw) {
        SelW *n = sw->next;
        struct RtSel *s = sw->sel;
        /* detached node refs are ours: s->t stays valid across the
         * claim; the RtSel ref (taken under ch->mu) keeps s itself */
        if (park_claim(s->t)) {
            s->winner = sw->slot;
            s->t->chan_res = val;
            s->t->sel_arm = sw->slot;
            rt_task_wake(s->t);
        }
        task_unref(s->t);
        sel_unref(s);
        sw = n;
    }
}

/* frame-task sleep: the delay lives on a detached timer thread so it
 * never occupies a work-pool worker. The sleeper parks on its scope's
 * parked list as WK_SLEEP so scope_cancel_wake can claim+wake it — a
 * sibling trap no longer waits out the timer (the sleep_ms contract).
 * The timer holds its own task ref and races the cancel claim like a
 * losing select arm: a failed park_claim means the winner already
 * dequeued/woke the task, so the timer just drops its ref and exits —
 * it can never touch freed task memory. */
typedef struct RtSleep {
    RtTask *t;     /* sleeper — the timer holds one ref */
    int64_t delay; /* timer units: seconds or microseconds (usec flag) */
    int usec;
} RtSleep;

static void *sleep_tick(void *p) {
    RtSleep *tm = p;
    if (tm->usec) { if (tm->delay > 0) sleep_us(tm->delay); }
    else if (tm->delay > 0) sleep_us(tm->delay * 1000000);
    RtTask *w = tm->t;
    free(tm);
    if (park_claim(w)) {
        park_del(w);
        rt_task_wake(w);
    }
    task_unref(w); /* the timer's own ref — w was live through the CAS */
    return 0;
}

static int sleep_poll(int64_t delay, int usec, RtTask *t) {
    /* cancel check + registration under one s->mu hold: a wake pass
     * that runs before park_add leaves no wake behind, and the timer
     * becomes the only waker — scope completion would wait out the
     * whole delay for a sibling trap that should exit fast */
    RtScope *s = t->scope;
    if (s) {
        oxl_mu_lock(&s->mu);
        if (s->cancel) {
            oxl_mu_unlock(&s->mu);
            return RT_READY; /* the post-poll rt_cancelled check dies */
        }
        task_ref(t);
        t->parked = 1;
        t->park_scope = s;
        t->waitobj = NULL;
        t->waitkind = WK_SLEEP;
        t->pnext = s->parked;
        s->parked = t;
        oxl_mu_unlock(&s->mu);
    } else {
        park_add(t, NULL, NULL, WK_SLEEP);
    }
    RtSleep *tm = xmalloc(sizeof *tm);
    tm->t = t;
    tm->delay = delay;
    tm->usec = usec;
    task_ref(t); /* the timer's ref — outlives an early settle */
    /* register BEFORE creating the thread: a timer that fires
     * instantly must find parked==1 to claim; waitobj stays NULL —
     * nothing to unlink, the timer self-cleans on a lost claim */
    oxl_thread_t th;
    if (oxl_thread_spawn(&th, NULL, sleep_tick, tm) != 0) {
        /* no thread: retract the registration and run inline */
        if (park_claim(t)) park_del(t);
        task_unref(t);
        free(tm);
        return RT_READY;
    }
    oxl_thread_detach(th);
    return RT_PENDING;
}

int rt_sleep_poll(int64_t secs, RtTask *t) {
    return sleep_poll(secs, 0, t);
}

int rt_msleep_poll(int64_t ms, RtTask *t) {
    return sleep_poll(ms * 1000, 1, t);
}

/* ---------------- select ----------------
 * A parked select owns one SelW node per recv arm, linked on each chan's
 * selq. The first fire claims the task's parked flag (park_claim) so a
 * loser arm's node can never double-wake: nodes that lose the CAS are
 * dead and get skipped by later senders. The woken task unlinks whatever
 * is still linked in rt_select_arm. */

/* under ch->mu (all arm chans locked): can this chan deliver now? */
static int sel_ready(RtChan *ch, int64_t *v, RtTask **wake_me) {
    if (ch->count > 0) { /* buffered: drain oldest, refill from sendq */
        *v = chan_ring_pop(ch, wake_me);
        return 1;
    }
    if (ch->sendq) {
        SendReq *r = ch->sendq;
        ch->sendq = r->next;
        *v = r->v;
        if (park_claim(r->t)) {
            park_del(r->t);
            *wake_me = r->t; /* caller wakes + drops the node ref */
        } else {
            task_unref(r->t); /* dead node — its ref dies here */
        }
        free(r);
        return 1;
    }
    if (ch->has || ch->closed) {
        *v = ch->has ? ch->val : 0;
        ch->has = 0;
        ch->val = 0;
        oxl_cv_broadcast(&ch->cv_send);
        return 1;
    }
    return 0;
}

static void *sel_timer(void *p) {
    struct RtSel *s = p;
    sleep_us(s->tmo * 1000);
    if (__sync_bool_compare_and_swap(&s->winner, -1, RT_SEL_TMO)
        && park_claim(s->t)) {
        s->t->sel_arm = RT_SEL_TMO;
        rt_task_wake(s->t);
    }
    sel_unref(s);
    return 0;
}

static int addr_cmp(const void *a, const void *b) {
    RtChan *x = *(RtChan *const *)a, *y = *(RtChan *const *)b;
    return (x > y) - (x < y);
}

int rt_select_poll(RtChan **chs, int n, int64_t tmo_ms, int has_else, RtTask *t) {
    /* fast path: peek the ready set under per-chan locks, pick
     * uniformly from it, then consume just that arm. A random-start
     * first-ready sweep is NOT uniform — an arm right after the start
     * point wins over equally-ready others (positional starvation the
     * spec forbids). Peek vs consume can disagree under contention, so
     * retries are bounded and the locked slow path settles the rest. */
    for (int attempt = 0; attempt < 4; attempt++) {
        int nready = 0;
        for (int i = 0; i < n; i++) {
            oxl_mu_lock(&chs[i]->mu);
            if (chs[i]->count > 0 || chs[i]->sendq || chs[i]->has || chs[i]->closed)
                nready++;
            oxl_mu_unlock(&chs[i]->mu);
        }
        if (nready == 0)
            break;
        int target = nready > 1 ? (int)(rand() % (unsigned)nready) : 0;
        int seen = 0;
        for (int i = 0; i < n; i++) {
            oxl_mu_lock(&chs[i]->mu);
            int rd = chs[i]->count > 0 || chs[i]->sendq || chs[i]->has || chs[i]->closed;
            if (!rd || seen++ != target) {
                oxl_mu_unlock(&chs[i]->mu);
                continue;
            }
            int64_t v = 0;
            RtTask *wm = NULL;
            int fired = sel_ready(chs[i], &v, &wm);
            oxl_mu_unlock(&chs[i]->mu);
            if (fired) {
                if (wm) { rt_task_wake(wm); task_unref(wm); }
                t->chan_res = v;
                t->sel_arm = i;
                return RT_READY;
            }
            break; /* peek lied between passes — recount and repick */
        }
    }
    if (has_else) {
        /* else must never park — after the bounded retries take any
         * arm that still fires, and else only when none does */
        for (int i = 0; i < n; i++) {
            oxl_mu_lock(&chs[i]->mu);
            int64_t v = 0;
            RtTask *wm = NULL;
            if (sel_ready(chs[i], &v, &wm)) {
                oxl_mu_unlock(&chs[i]->mu);
                if (wm) { rt_task_wake(wm); task_unref(wm); }
                t->chan_res = v;
                t->sel_arm = i;
                return RT_READY;
            }
            oxl_mu_unlock(&chs[i]->mu);
        }
        t->sel_arm = RT_SEL_ELSE;
        return RT_READY;
    }

    /* slow path: lock every arm chan in address order, re-check under
     * all locks, then push a waiter node on each still-unready chan */
    RtChan **sorted = xmalloc(sizeof *sorted * (size_t)n);
    memcpy(sorted, chs, sizeof *sorted * (size_t)n);
    qsort(sorted, (size_t)n, sizeof *sorted, addr_cmp);
    /* two arms may name the same chan — it sorts adjacent, so lock
     * each distinct address once or the re-lock self-deadlocks */
    for (int i = 0; i < n; i++)
        if (i == 0 || sorted[i] != sorted[i - 1])
            oxl_mu_lock(&sorted[i]->mu);

    /* re-check: a send may have landed between the sweep and the lock.
     * Peek the ready set — sel_ready consumes (pops ring/sendq, claims
     * senders), so a destructive scan would drop payloads and orphan
     * every claimed sender but the last. Peeked state can't change
     * while every arm lock is held, so the picked chan still fires. */
    int nready = 0;
    for (int i = 0; i < n; i++)
        if (chs[i]->count > 0 || chs[i]->sendq || chs[i]->has || chs[i]->closed)
            nready++;
    if (nready) {
        int pick = nready > 1 ? (int)(rand() % (unsigned)nready) : 0;
        int64_t v = 0;
        RtTask *wm = NULL;
        int arm = -1, seen = 0;
        for (int i = 0; i < n; i++) {
            if (chs[i]->count > 0 || chs[i]->sendq || chs[i]->has || chs[i]->closed) {
                if (seen++ == pick) { sel_ready(chs[i], &v, &wm); arm = i; break; }
            }
        }
        for (int i = 0; i < n; i++)
            if (i == 0 || sorted[i] != sorted[i - 1])
                oxl_mu_unlock(&sorted[i]->mu);
        free(sorted);
        if (wm) { rt_task_wake(wm); task_unref(wm); }
        t->chan_res = v;
        t->sel_arm = arm;
        return RT_READY;
    }

    /* park: claim a pending flag first, then hang a node on every arm */
    struct RtSel *s = xmalloc(sizeof *s + sizeof(SelW) * (size_t)n);
    s->winner = -1;
    s->refs = 1 + (tmo_ms >= 0 ? 1 : 0);
    s->n = n;
    s->tmo = tmo_ms;
    s->t = t;
    task_ref(t); /* the RtSel's ref — released when s is freed */
    t->sel = s;
    park_add(t, t->scope, s, WK_SEL);
    for (int i = 0; i < n; i++) {
        s->w[i].sel = s;
        s->w[i].slot = i;
        RtChan *ch = chs[i];
        s->w[i].ch = ch;
        s->w[i].next = ch->selq;
        ch->selq = &s->w[i];
        task_ref(t); /* one ref per selq node — leaves with the node */
    }
    for (int i = 0; i < n; i++)
        if (i == 0 || sorted[i] != sorted[i - 1])
            oxl_mu_unlock(&sorted[i]->mu);
    free(sorted);
    if (tmo_ms >= 0) {
        oxl_thread_t th;
        if (oxl_thread_spawn(&th, NULL, sel_timer, s) == 0) {
            oxl_thread_detach(th);
        } else {
            /* no timer thread: fire the timeout arm ourselves so the
             * task isn't parked forever; drop the ref we pre-armed */
            if (__sync_bool_compare_and_swap(&s->winner, -1, RT_SEL_TMO)
                && park_claim(t)) {
                t->sel_arm = RT_SEL_TMO;
                rt_task_wake(t);
            }
            sel_unref(s); /* drop the pre-armed timer ref */
            return RT_PENDING;
        }
    }
    return RT_PENDING;
}

/* resume path: unlink the parked waiters, free the RtSel (ref-shared
 * with an armed timer), and report which arm fired. */
int64_t rt_select_arm(RtTask *t) {
    int64_t arm = t->sel_arm;
    struct RtSel *s = t->sel;
    t->sel = NULL;
    if (!s) return arm;
    /* every WK_SEL waker claims the parked flag but never unlinks —
     * without this the freed task stays on scope->parked (UAF) */
    park_del(t);
    for (int i = 0; i < s->n; i++) {
        RtChan *ch = s->w[i].ch;
        int unlinked = 0;
        oxl_mu_lock(&ch->mu);
        SelW **pp = &ch->selq;
        while (*pp) {
            if (*pp == &s->w[i]) { *pp = s->w[i].next; unlinked = 1; break; }
            pp = &(*pp)->next;
        }
        oxl_mu_unlock(&ch->mu);
        /* a node already popped (waker/close-drain) carries its ref
         * to whoever removed it — only ours to drop when we unlink */
        if (unlinked) task_unref(t);
    }
    sel_unref(s);
    return arm;
}

/* scope cancel: force-wake every task parked inside `s` so suspended
 * tasks observe cancel at their resume point instead of hanging the
 * join. Claims the parked flag first — whoever won the CAS owns the
 * wake; queue unlink is hygiene. Thread waits get a cv broadcast and
 * unwind on their own cancel check. */
static void scope_cancel_wake(RtScope *s) {
    for (;;) {
        oxl_mu_lock(&s->mu);
        RtTask *t = s->parked;
        if (t) s->parked = t->pnext;
        oxl_mu_unlock(&s->mu);
        if (!t) return;
        /* the pop hands us the node's parked-list ref: t stays valid
         * through our claim+wake and the ref drops at loop bottom */
        /* joiners stay parked: they wake via scope_done when pending
         * drains — a premature finish would consume a live scope */
        if (t->waitkind == WK_JOIN) { task_unref(t); continue; }
        if (!park_claim(t)) { task_unref(t); continue; }
        switch (t->waitkind) {
            case WK_RECV: {
                RtChan *ch = t->waitobj;
                int unlinked = 0;
                oxl_mu_lock(&ch->mu);
                RtTask **pp = &ch->recvq;
                while (*pp) {
                    if (*pp == t) { *pp = t->next; unlinked = 1; break; }
                    pp = &(*pp)->next;
                }
                oxl_mu_unlock(&ch->mu);
                if (unlinked) task_unref(t); /* recvq node ref */
                break;
            }
            case WK_SEND: {
                RtChan *ch = t->waitobj;
                int unlinked = 0;
                oxl_mu_lock(&ch->mu);
                SendReq **pp = &ch->sendq;
                while (*pp) {
                    if ((*pp)->t == t) {
                        SendReq *d = *pp;
                        *pp = d->next;
                        free(d);
                        unlinked = 1;
                        break;
                    }
                    pp = &(*pp)->next;
                }
                oxl_mu_unlock(&ch->mu);
                if (unlinked) task_unref(t); /* sendq node ref */
                break;
            }
            case WK_SEL: {
                struct RtSel *sel = t->waitobj;
                /* fence off a live timer: with winner still -1 its CAS
                 * would claim a task we've already woken (→ freed) */
                __sync_bool_compare_and_swap(&sel->winner, -1, RT_SEL_ELSE);
                for (int i = 0; i < sel->n; i++) {
                    RtChan *ch = sel->w[i].ch;
                    int unlinked = 0;
                    oxl_mu_lock(&ch->mu);
                    SelW **pp = &ch->selq;
                    while (*pp) {
                        if (*pp == &sel->w[i]) { *pp = sel->w[i].next; unlinked = 1; break; }
                        pp = &(*pp)->next;
                    }
                    oxl_mu_unlock(&ch->mu);
                    if (unlinked) task_unref(t); /* selq node ref */
                }
                break;
            }
            case WK_JOIN: {
                RtScope *js = t->waitobj;
                oxl_mu_lock(&js->mu);
                if (js->join_task == t) js->join_task = NULL;
                oxl_mu_unlock(&js->mu);
                break;
            }
            case WK_SLEEP:
                /* nothing to unlink: the timer thread races this same
                 * claim and self-cleans on the loss */
                break;
            case WK_TRECV: case WK_TSEND: {
                RtChan *ch = t->waitobj;
                oxl_mu_lock(&ch->mu);
                oxl_cv_broadcast(&ch->cv_recv);
                oxl_cv_broadcast(&ch->cv_send);
                oxl_mu_unlock(&ch->mu);
                task_unref(t);
                continue; /* thread wakes on its cv, exits on cancel check */
            }
        }
        rt_task_wake(t);
        task_unref(t);
    }
}

/* copy s's backing into own-heap storage — a closure env holds the
 * header past the creating scope's arena teardown, so the bytes must
 * live in the same escaping storage the env does */
OxlStr rt_str_own(OxlStr s) {
    char *p = rt_alloc_own(s.n + 1);
    memcpy(p, s.p, (size_t)s.n);
    p[s.n] = 0;
    return (OxlStr){ p, s.n };
}

OxlStr rt_str(const char *s) { return (OxlStr){ s, (int64_t)strlen(s) }; }
OxlStr rt_str_lit(const char *s) { return rt_str(s); }

OxlStr rt_strcat(OxlStr a, OxlStr b) {
    char *p = rt_alloc(a.n + b.n + 1);
    memcpy(p, a.p, a.n);
    memcpy(p + a.n, b.p, b.n);
    p[a.n + b.n] = 0;
    return (OxlStr){ p, a.n + b.n };
}

int rt_streq(OxlStr a, OxlStr b) {
    return a.n == b.n && memcmp(a.p, b.p, (size_t)a.n) == 0;
}

OxlStr rt_itoa(int64_t v) {
    char *p = rt_alloc(24);
    snprintf(p, 24, "%lld", (long long)v);
    return rt_str(p);
}

/* ---------------- slices ({ptr,len,cap,heap} headers, B3) ----------------
 * Borrow slices use the scope arena (bulk-freed at scope end); `own`
 * slices get a heap backing via rt_slice_own so the header can escape
 * its scope without dangling. The `heap` flag travels the header so
 * views and growth preserve the backing's domain. */

RtSlice rt_mkslice(int64_t n, int64_t esz) {
    RtSlice s;
    s.len = n;
    s.cap = n;
    s.heap = 0;
    s.ptr = n > 0 ? rt_alloc(n * esz) : 0;
    if (s.ptr) memset(s.ptr, 0, (size_t)(n * esz));
    return s;
}

RtSlice rt_slice_push(RtSlice s, int64_t esz, void *v) {
    if (s.len >= s.cap) {
        int64_t ncap = s.cap > 0 ? s.cap * 2 : 4;
        /* growth keeps the backing's domain: arena stays scope-lived,
         * heap stays escape-safe */
        void *nb = s.heap ? rt_alloc_own(ncap * esz) : rt_alloc(ncap * esz);
        if (s.ptr) memcpy(nb, s.ptr, (size_t)(s.len * esz));
        s.ptr = nb;
        s.cap = ncap;
    }
    memcpy((char *)s.ptr + s.len * esz, v, (size_t)esz);
    s.len++;
    return s;
}

/* own []T: boxed header + private heap copy of the backing — escaping
 * the scope can't leave the elements pointing into a freed arena */
RtSlice *rt_slice_own(RtSlice s, int64_t esz) {
    RtSlice *b = rt_alloc_own(sizeof *b);
    *b = s;
    if (s.ptr && s.cap > 0) {
        void *nb = rt_alloc_own(s.cap * esz);
        memcpy(nb, s.ptr, (size_t)(s.len * esz));
        b->ptr = nb;
    }
    b->heap = 1;
    return b;
}

const char *rt_cstr(OxlStr s) {
    char *p = rt_alloc(s.n + 1);
    memcpy(p, s.p, s.n);
    p[s.n] = 0;
    return p;
}

static void wr(OxlStr s) { fwrite(s.p, 1, (size_t)s.n, stdout); }
void rt_print_str(OxlStr s) { wr(s); }
void rt_println_str(OxlStr s) { wr(s); wr(rt_str("\n")); fflush(stdout); }
void rt_print_i64(int64_t v) { printf("%lld", (long long)v); }
void rt_println_i64(int64_t v) { printf("%lld\n", (long long)v); fflush(stdout); }
void rt_print_f64(double v) { printf("%.15g", v); }
void rt_println_f64(double v) { printf("%.15g\n", v); fflush(stdout); }

void rt_sleep(int64_t secs) {
    if (rt_cancelled()) task_unwind();
    sleep_us(secs * 1000000);
}

/* ms-granularity variant — same suspend contract as rt_sleep */
void rt_msleep(int64_t ms) {
    if (rt_cancelled()) task_unwind();
    if (ms > 0) sleep_us(ms * 1000);
}

/* ---------------- args, strings, buffers ---------------- */

static int g_argc;
static char **g_argv;
void rt_set_args(int argc, char **argv) { g_argc = argc; g_argv = argv; }
int64_t rt_argc(void) { return g_argc; }
OxlStr rt_argv(int64_t i) {
    if (i < 0 || i >= g_argc) return rt_str("");
    return rt_str(g_argv[i]);
}

int64_t rt_len(OxlStr s) { return s.n; }
int64_t rt_byteat(OxlStr s, int64_t i) {
    if (i < 0 || i >= s.n) return -1;
    return (uint8_t)s.p[i];
}
OxlStr rt_substr(OxlStr s, int64_t a, int64_t b) {
    if (a < 0) a = 0;
    if (b > s.n) b = s.n;
    if (a >= b) return rt_str("");
    char *p = rt_alloc(b - a + 1);
    memcpy(p, s.p + a, b - a);
    p[b - a] = 0;
    return (OxlStr){ p, b - a };
}
int64_t rt_find(OxlStr s, OxlStr sub) {
    for (int64_t i = 0; i + sub.n <= s.n; i++)
        if (memcmp(s.p + i, sub.p, (size_t)sub.n) == 0) return i;
    return -1;
}
int64_t rt_atoi(OxlStr s) { return strtoll(rt_cstr(s), NULL, 10); }

double rt_atof(OxlStr s) { return strtod(rt_cstr(s), NULL); }

OxlStr rt_ftoa(double v) {
    char *p = rt_alloc(40);
    snprintf(p, 40, "%.15g", v);
    return rt_str(p);
}

void *rt_mkbuf(int64_t size) { return rt_alloc_own(size); }
OxlStr rt_bufstr(void *buf, int64_t n) {
    char *p = rt_alloc(n + 1);
    memcpy(p, buf, (size_t)n);
    p[n] = 0;
    return (OxlStr){ p, n };
}
void rt_poke8(void *buf, int64_t off, int64_t v) { ((uint8_t *)buf)[off] = (uint8_t)v; }
void rt_poke16(void *buf, int64_t off, int64_t v) {
    uint16_t x = htons((uint16_t)v);
    memcpy((uint8_t *)buf + off, &x, sizeof x);
}
void rt_poke32(void *buf, int64_t off, int64_t v) {
    uint32_t x = htonl((uint32_t)v);
    memcpy((uint8_t *)buf + off, &x, sizeof x);
}
int64_t rt_htons(int64_t v) { return htons((uint16_t)v); }
int64_t rt_htonl(int64_t v) { return htonl((uint32_t)v); }

/* ---------------- file io + process ---------------- */

OxlStr rt_readfile(OxlStr path) {
    char *cp = rt_alloc(path.n + 1);
    memcpy(cp, path.p, (size_t)path.n);
    cp[path.n] = 0;
    FILE *f = fopen(cp, "rb");
    if (!f) return rt_str("");
    fseek(f, 0, SEEK_END);
    long n = ftell(f);
    if (n < 0) { fclose(f); return rt_str(""); }
    fseek(f, 0, SEEK_SET);
    char *buf = rt_alloc(n + 1);
    n = (long)fread(buf, 1, (size_t)n, f);
    fclose(f);
    buf[n] = 0;
    return (OxlStr){ buf, n };
}

int64_t rt_writefile(OxlStr path, OxlStr data) {
    char *cp = rt_alloc(path.n + 1);
    memcpy(cp, path.p, (size_t)path.n);
    cp[path.n] = 0;
    FILE *f = fopen(cp, "wb");
    if (!f) return -1;
    size_t w = fwrite(data.p, 1, (size_t)data.n, f);
    int cerr = fclose(f);
    return (w == (size_t)data.n && cerr == 0) ? 0 : -1;
}

void rt_exit(int64_t code) { exit((int)code); }

/* portable socket setup — SOL_SOCKET/SO_REUSEADDR constants and the
 * sockaddr_in layout differ across Linux/macOS/BSD (sin_len exists only
 * on BSD-derived systems), so they can't be spelled in oxl source */
int64_t rt_set_reuseaddr(int64_t fd) {
    int one = 1;
    return setsockopt((oxl_sock_t)fd, SOL_SOCKET, SO_REUSEADDR, (const char *)&one, sizeof one);
}

int64_t rt_bind_any(int64_t fd, int64_t port) {
    struct sockaddr_in a;
    memset(&a, 0, sizeof a);
    a.sin_family = AF_INET;
    a.sin_port = htons((uint16_t)port);
    a.sin_addr.s_addr = htonl(INADDR_ANY);
    return bind((oxl_sock_t)fd, (struct sockaddr *)&a, sizeof a);
}

/* portable receive timeout — SO_RCVTIMEO's constant differs by OS */
int64_t rt_set_rcvtimo(int64_t fd, int64_t secs) {
#ifdef _WIN32
    DWORD tv = (DWORD)(secs * 1000); /* ms granularity on WinSock */
    return setsockopt((oxl_sock_t)fd, SOL_SOCKET, SO_RCVTIMEO, (const char *)&tv, sizeof tv);
#else
    struct timeval tv;
    tv.tv_sec = (time_t)secs;
    tv.tv_usec = 0;
    return setsockopt((int)fd, SOL_SOCKET, SO_RCVTIMEO, &tv, sizeof tv);
#endif
}

/* portable send timeout — pairs with rcvtimo: a client that stops
 * reading must not pin a connection slot inside write() */
int64_t rt_set_sndtimo(int64_t fd, int64_t secs) {
#ifdef _WIN32
    DWORD tv = (DWORD)(secs * 1000);
    return setsockopt((oxl_sock_t)fd, SOL_SOCKET, SO_SNDTIMEO, (const char *)&tv, sizeof tv);
#else
    struct timeval tv;
    tv.tv_sec = (time_t)secs;
    tv.tv_usec = 0;
    return setsockopt((int)fd, SOL_SOCKET, SO_SNDTIMEO, &tv, sizeof tv);
#endif
}

/* file readability probe — readfile()'s "" result can't tell an
 * empty file from an unreadable one; callers that care probe first.
 * fopen("rb") is the portable open(O_RDONLY) — no read needed */
int64_t rt_file_readable(const char *path) {
    FILE *f = fopen(path, "rb");
    if (!f) return 0;
    fclose(f);
    return 1;
}

/* socket fd I/O — POSIX read/write/close have no socket counterpart
 * on Win32 (recv/send/closesocket live in ws2_32), so .oxl code names
 * these instead of libc directly */
int64_t rt_sock_read(int64_t fd, void *buf, int64_t n) {
#ifdef _WIN32
    return recv((SOCKET)fd, (char *)buf, (int)n, 0);
#else
    return read((int)fd, buf, (size_t)n);
#endif
}

int64_t rt_sock_write(int64_t fd, const char *buf, int64_t n) {
#ifdef _WIN32
    return send((SOCKET)fd, (const char *)buf, (int)n, 0);
#else
    return write((int)fd, buf, (size_t)n);
#endif
}

int64_t rt_sock_close(int64_t fd) {
#ifdef _WIN32
    return closesocket((SOCKET)fd);
#else
    return close((int)fd);
#endif
}

/* process-wide runtime init — WinSock needs WSAStartup before the
 * first socket call; POSIX has no equivalent */
void rt_init(void) {
#ifdef _WIN32
    WSADATA wd;
    if (WSAStartup(MAKEWORD(2, 2), &wd) != 0)
        fprintf(stderr, "oxl: WSAStartup failed\n");
#endif
}

/* ---------------- sqlite3 shims ----------------
 * Built only under -DOXL_SQLITE: the compiler defines it (and links
 * -lsqlite3) when the emitted program references rt_sqlite_*, so a
 * stock system without the sqlite dev headers can still build oxl
 * programs that never touch a database. */
#ifdef OXL_SQLITE

#include <sqlite3.h>

void *rt_sqlite_open(const char *path) {
    sqlite3 *db = NULL;
    if (sqlite3_open(path, &db) != SQLITE_OK) {
        /* sqlite3_open can allocate a handle even on failure — it owns
         * the error text, so report through it before closing */
        if (db) {
            fprintf(stderr, "sqlite open failed: %s\n", sqlite3_errmsg(db));
            sqlite3_close(db);
        }
        return NULL;
    }
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
        sleep_us(10000);
    }
    return SQLITE_BUSY;
}
/* prepared-statement insert for request-supplied values — body text is
 * bound, never concatenated into SQL */
int64_t rt_sqlite_exec_bind(void *db, const char *sql, OxlStr param) {
    sqlite3_stmt *st = NULL;
    for (int i = 0; i < 500; i++) {
        if (sqlite3_prepare_v2((sqlite3 *)db, sql, -1, &st, NULL) != SQLITE_OK)
            return -1;
        sqlite3_bind_text(st, 1, param.p, (int)param.n, SQLITE_TRANSIENT);
        int rc = sqlite3_step(st);
        sqlite3_finalize(st);
        if (rc != SQLITE_BUSY && rc != SQLITE_LOCKED)
            return rc == SQLITE_DONE ? 0 : rc;
        sleep_us(10000);
    }
    return SQLITE_BUSY;
}
OxlStr rt_sqlite_query(void *db, const char *sql) {
    sqlite3_stmt *st = NULL;
    if (sqlite3_prepare_v2((sqlite3 *)db, sql, -1, &st, NULL) != SQLITE_OK)
        return rt_str("");
    /* rt_strcat re-copies the prefix per field — quadratic on wide or
     * long result sets. Accumulate in a doubling scratch, then pay one
     * copy into scope memory for the result. */
    size_t cap = 4096, n = 0;
    char *buf = xmalloc(cap);
    while (sqlite3_step(st) == SQLITE_ROW) {
        int cols = sqlite3_column_count(st);
        for (int c = 0; c < cols; c++) {
            const unsigned char *t = sqlite3_column_text(st, c);
            size_t tl = t ? strlen((const char *)t) : 0;
            if (n + tl + 2 > cap) { /* field + separator + slack */
                while (n + tl + 2 > cap) cap *= 2;
                buf = xrealloc(buf, cap);
            }
            if (tl) { memcpy(buf + n, t, tl); n += tl; }
            if (c + 1 < cols) buf[n++] = '|';
        }
        buf[n++] = '\n';
    }
    sqlite3_finalize(st);
    char *p = rt_alloc((int64_t)n + 1);
    memcpy(p, buf, n);
    p[n] = 0;
    free(buf);
    return (OxlStr){ p, (int64_t)n };
}
void rt_sqlite_close(void *db) { sqlite3_close((sqlite3 *)db); }

#endif /* OXL_SQLITE */

/* ---------------- entry ---------------- */

int main(int argc, char **argv) {
    rt_init();
    /* select's uniform pick draws on rand() — seed per-process or every
     * run replays the same arm-pick sequence */
    srand((unsigned)(time(NULL) ^ oxl_pid()));
    rt_set_args(argc, argv);
    RtScope base;
    rt_scope_enter(&base);
    oxl_main();
    rt_scope_leave(&base);
    return 0;
}
