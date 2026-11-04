#!/usr/bin/env bash
# build the gui-todo pilot: shim with gtk4 cflags, oxl app linked
# against it via LANG_LDFLAGS
set -euo pipefail
cd "$(dirname "$0")"

OXL=${OXL:-../../oxl/target/release/oxl}
GTK_CFLAGS=$(pkg-config --cflags gtk4)
GTK_LIBS=$(pkg-config --libs gtk4)

${LANG_CC:-cc} -O2 $GTK_CFLAGS -c gui_shim.c -o gui_shim.o
LANG_LDFLAGS="$PWD/gui_shim.o $GTK_LIBS" "$OXL" build todo.oxl -o todo
echo "built ./todo"
