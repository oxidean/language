/* gui_shim.c — C ABI shim between OXL and GTK4.
 *
 * Why a shim exists (spec F1): GTK's signal API is macro-based
 * (g_signal_connect expands G_CALLBACK casts, GTK_WIDGET/GTK_EDITABLE
 * casts), which `extern fn` cannot express. Every function here is a
 * plain C symbol taking/returning scalar values — OXL calls them via
 * `extern fn` decls in todo.oxl, and this file is compiled once with
 * `pkg-config --cflags gtk4` and linked via LANG_LDFLAGS.
 *
 * Handles are int64_t — opaque widget pointers carried through OXL's
 * `int` type, the same idiom the http stdlib uses for socket fds.
 * OXL closures arrive as RtFnv {env, fp}; the trampoline calls
 * fp(env, widget) — the uniform (env, args...) fn-value convention.
 */
#include <gtk/gtk.h>
#include <stdint.h>
#include <stdlib.h>
#include "../../runtime/rt.h" /* RtFnv layout + rt_free for cb envs */

static GMainLoop *the_loop;

/* --- init / window / main loop --- */

void gui_init(void) { gtk_init(); }

static gboolean on_close_req(GtkWindow *w, gpointer data) {
    (void)w; (void)data;
    g_main_loop_quit(the_loop);
    return FALSE; /* let GTK destroy the window */
}

int64_t gui_window_new(const char *title, int64_t w, int64_t h) {
    GtkWidget *win = gtk_window_new();
    gtk_window_set_title(GTK_WINDOW(win), title);
    gtk_window_set_default_size(GTK_WINDOW(win), (int)w, (int)h);
    /* close-request → quit the loop; this is the whole "app quit"
     * story for the pilot (no GtkApplication plumbing) */
    g_signal_connect(win, "close-request", G_CALLBACK(on_close_req), NULL);
    return (int64_t)(intptr_t)win;
}

void gui_window_present(int64_t win) {
    gtk_window_present(GTK_WINDOW((gpointer)(intptr_t)win));
}

int gui_run(void) {
    the_loop = g_main_loop_new(NULL, FALSE);
    g_main_loop_run(the_loop);
    g_main_loop_unref(the_loop);
    return 0;
}

/* selftest support: quit the loop after ms so the app can drive
 * itself in CI/headless verification */
static gboolean quit_cb(gpointer data) {
    (void)data;
    g_main_loop_quit(the_loop);
    return G_SOURCE_REMOVE;
}
void gui_quit_after(int64_t ms) { g_timeout_add((guint)ms, quit_cb, NULL); }

/* --- widgets --- */

int64_t gui_box_new(int64_t vertical, int64_t spacing) {
    return (int64_t)(intptr_t)gtk_box_new(
        vertical ? GTK_ORIENTATION_VERTICAL : GTK_ORIENTATION_HORIZONTAL,
        (int)spacing);
}
void gui_box_append(int64_t box, int64_t child) {
    gtk_box_append(GTK_BOX((gpointer)(intptr_t)box),
                   GTK_WIDGET((gpointer)(intptr_t)child));
}
void gui_box_remove(int64_t box, int64_t child) {
    gtk_box_remove(GTK_BOX((gpointer)(intptr_t)box),
                   GTK_WIDGET((gpointer)(intptr_t)child));
}

int64_t gui_entry_new(void) {
    return (int64_t)(intptr_t)gtk_entry_new();
}
/* borrowed pointer — valid until the entry's text changes */
const char *gui_entry_text(int64_t entry) {
    return gtk_editable_get_text(GTK_EDITABLE((gpointer)(intptr_t)entry));
}
void gui_entry_set_text(int64_t entry, const char *t) {
    gtk_editable_set_text(GTK_EDITABLE((gpointer)(intptr_t)entry), t);
}

int64_t gui_button_new(const char *label) {
    return (int64_t)(intptr_t)gtk_button_new_with_label(label);
}
int64_t gui_label_new(const char *text) {
    return (int64_t)(intptr_t)gtk_label_new(text);
}
void gui_label_set_text(int64_t lbl, const char *t) {
    gtk_label_set_text(GTK_LABEL((gpointer)(intptr_t)lbl), t);
}

int64_t gui_listbox_new(void) {
    return (int64_t)(intptr_t)gtk_list_box_new();
}
void gui_listbox_append(int64_t lb, int64_t row) {
    gtk_list_box_append(GTK_LIST_BOX((gpointer)(intptr_t)lb),
                        GTK_WIDGET((gpointer)(intptr_t)row));
}
void gui_listbox_remove(int64_t lb, int64_t row) {
    /* GTK wraps appended widgets in GtkListBoxRow — accept either the
     * row or any widget inside it (we hand OXL the inner box) */
    GtkWidget *w = (gpointer)(intptr_t)row;
    GtkWidget *r = GTK_IS_LIST_BOX_ROW(w)
        ? w : gtk_widget_get_ancestor(w, GTK_TYPE_LIST_BOX_ROW);
    if (r) gtk_list_box_remove(GTK_LIST_BOX((gpointer)(intptr_t)lb), r);
}
int64_t gui_listbox_rows(int64_t lb) {
    int64_t n = 0;
    GtkWidget *w = gtk_widget_get_first_child(
        GTK_WIDGET((gpointer)(intptr_t)lb));
    for (; w; w = gtk_widget_get_next_sibling(w)) n++;
    return n;
}

int64_t gui_scrolled_new(int64_t child) {
    GtkWidget *s = gtk_scrolled_window_new();
    gtk_scrolled_window_set_child(GTK_SCROLLED_WINDOW(s),
                                  GTK_WIDGET((gpointer)(intptr_t)child));
    gtk_widget_set_vexpand(s, TRUE);
    return (int64_t)(intptr_t)s;
}

/* --- signals: the only place GTK's macro API leaks through ---
 *
 * cb is an OXL fn value {env, fp} passed by value; we heap-copy it for
 * the signal's lifetime and free it (plus the OXL env, which rt_free
 * handles — arena envs are skipped) in the destroy notify.
 */
static void tramp(GtkWidget *w, gpointer data) {
    RtFnv *cb = data;
    ((void (*)(void *, GtkWidget *))cb->fp)(cb->env, w);
}

/* one RtFnv env can back several registrations (the same fn value
 * connected to two signals copies the env pointer) — refcount per env
 * so the last destroy notify frees it exactly once. GTK objects are
 * main-thread-confined, so no locking. */
typedef struct CbEnv {
    void *env;
    int refs;
    struct CbEnv *next;
} CbEnv;
static CbEnv *cb_envs;

static void cb_destroy(gpointer data, GClosure *cl) {
    (void)cl;
    RtFnv *cb = data;
    for (CbEnv **pp = &cb_envs; *pp; pp = &(*pp)->next) {
        if ((*pp)->env == cb->env) {
            if (--(*pp)->refs == 0) {
                CbEnv *d = *pp;
                *pp = d->next;
                rt_free(d->env);
                free(d);
            }
            free(cb);
            return;
        }
    }
    rt_free(cb->env); /* connect failed before registering */
    free(cb);
}
void gui_connect(int64_t obj, const char *sig, RtFnv cb) {
    RtFnv *heap = malloc(sizeof(RtFnv));
    *heap = cb;
    CbEnv *e = cb_envs;
    for (; e; e = e->next)
        if (e->env == cb.env) break;
    if (!e) {
        e = malloc(sizeof *e);
        e->env = cb.env;
        e->refs = 0;
        e->next = cb_envs;
        cb_envs = e;
    }
    e->refs++;
    g_signal_connect_data(G_OBJECT((gpointer)(intptr_t)obj), sig,
                          G_CALLBACK(tramp), heap, cb_destroy, 0);
}
/* selftest: emit a signal so handlers run through the real GTK path */
void gui_emit(int64_t obj, const char *sig) {
    g_signal_emit_by_name(G_OBJECT((gpointer)(intptr_t)obj), sig);
}
void gui_window_set_child(int64_t win, int64_t child) {
    gtk_window_set_child(GTK_WINDOW((gpointer)(intptr_t)win),
                         GTK_WIDGET((gpointer)(intptr_t)child));
}
