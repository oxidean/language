/* lang runtime — public surface for generated code.
 * Implements the contracts in docs/semantics/{ownership,concurrency}.md:
 * ObjHdr checks, scope arenas, scope-contained traps, task executor,
 * unbuffered ownership-transfer channels.
 */
#pragma once
#include <pthread.h>
#include <stdint.h>

typedef struct {
    const char *p;
    int64_t n;
} LangStr;

typedef struct {
    uint32_t gen;
    uint8_t borrow; /* 0 free, 1 read-held, 2 mut-held */
    uint8_t arena;
    uint8_t poisoned;
    uint8_t _pad;
} ObjHdr;

/* --- scopes & tasks (concurrency.md, interim pool impl) --- */
typedef struct RtScope {
    pthread_t *threads;
    int n, cap;
    volatile int trapped;
    volatile int cancel;
    int64_t err;
    struct RtScope *parent;
    /* arena: bump allocator freed on leave */
    char *arena_buf;
    int64_t arena_off, arena_cap;
} RtScope;

void rt_scope_enter(RtScope *s);
void rt_scope_leave(RtScope *s);
int rt_scope_try(RtScope *s); /* join; return trap code without propagating */
void rt_spawn(void *(*fn)(void *), void *arg);
void rt_trap(int64_t code);

/* --- allocation & ownership checks (ownership.md) --- */
void *rt_alloc(int64_t size);       /* scope arena if inside scope, else heap */
void *rt_alloc_own(int64_t size);   /* always heap (escaping data) */
void rt_borrow_read(void *obj);
void rt_borrow_mut(void *obj);
void rt_unborrow(void *obj);
void rt_free(void *obj);

/* --- channels --- */
typedef struct RtChan RtChan;
RtChan *rt_mkchan(void);
void rt_send(RtChan *ch, int64_t v);
int64_t rt_recv(RtChan *ch);
void rt_close(RtChan *ch);

/* --- misc builtins --- */
void rt_sleep(int64_t secs);
LangStr rt_str_lit(const char *s);
LangStr rt_strcat(LangStr a, LangStr b);
LangStr rt_itoa(int64_t v);
LangStr rt_str(const char *s);
void rt_print_str(LangStr s);
void rt_print_i64(int64_t v);
void rt_println_str(LangStr s);
void rt_println_i64(int64_t v);
const char *rt_cstr(LangStr s);

/* args + strings + byte buffers (stdlib wave-1 surface) */
void rt_set_args(int argc, char **argv);
int64_t rt_argc(void);
LangStr rt_argv(int64_t i);
int64_t rt_len(LangStr s);
int64_t rt_byteat(LangStr s, int64_t i);
LangStr rt_substr(LangStr s, int64_t a, int64_t b);
int64_t rt_find(LangStr s, LangStr sub);
int64_t rt_atoi(LangStr s);
void *rt_mkbuf(int64_t size);
LangStr rt_bufstr(void *buf, int64_t n);
void rt_poke8(void *buf, int64_t off, int64_t v);
void rt_poke16(void *buf, int64_t off, int64_t v);
void rt_poke32(void *buf, int64_t off, int64_t v);
int64_t rt_htons(int64_t v);
int64_t rt_htonl(int64_t v);

/* sqlite3 shims — out-param (T**) FFI needs these until the type
 * system gains address-of; direct calls (strlen etc.) use `extern` */
void *rt_sqlite_open(const char *path);
int64_t rt_sqlite_exec(void *db, const char *sql);
LangStr rt_sqlite_query(void *db, const char *sql);
void rt_sqlite_close(void *db);

/* entry: generated code provides lang_main() */
void lang_main(void);
