#!/usr/bin/env bash
# build the gui-todo pilot: shim with gtk4 cflags, oxl app linked
# against it via LANG_LDFLAGS
set -euo pipefail
cd "$(dirname "$0")"

if [ -z "${OXL:-}" ]; then
    # source checkout builds into oxl/target/release; the packaged
    # toolchain drops `oxl` at the root next to stdlib/ + examples/
    if [ -x ../../oxl/target/release/oxl ]; then
        OXL=../../oxl/target/release/oxl
    else
        OXL=../../oxl
    fi
fi
GTK_CFLAGS=$(pkg-config --cflags gtk4)
GTK_LIBS=$(pkg-config --libs gtk4)

"${LANG_CC:-cc}" -O2 $GTK_CFLAGS -c gui_shim.c -o gui_shim.o
# relative shim path: LANG_LDFLAGS is whitespace-split by the driver, so an
# absolute path breaks on checkouts whose directory contains spaces
LANG_LDFLAGS="gui_shim.o $GTK_LIBS" "$OXL" build todo.oxl -o todo
echo "built ./todo"
