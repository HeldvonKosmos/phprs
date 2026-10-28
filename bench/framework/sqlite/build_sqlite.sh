#!/usr/bin/env bash
# SQLite of the framework benchmark (D23, BENCH3): the SQLite that Bun 1.4.3
# builds into bun:sqlite (Jonas, 28.10.: every side on Bun's version), as a
# shared library: Bun's amalgamation of SQLite 3.53.4 (the official one with
# the parser regenerated for SQLITE_ENABLE_UPDATE_DELETE_LIMIT) with Bun's
# defines (scripts/build/deps/sqlite.ts of bun-v1.4.3); the headers from the
# official amalgamation of the same version. Every other side loads or links
# this library (OpenGrid, CPython, .NET through LD_LIBRARY_PATH; Ruby, Java,
# Phoenix, Go, the self-built FrankenPHP linked against it); check_sqlite.py
# and bm2.py compare every side's `pragma compile_options` with it.
#   bench/framework/sqlite/build_sqlite.sh   -> ~/.local/opt/sqlite-3.53.4/{lib,include,src}
# The amalgamation, headers and defines stay in <prefix>/src.
set -euo pipefail
VER=3.53.4
BUN_TAG=bun-v1.4.3
BUN_C_SHA=880b659cd7de38de61d747ac539c734e46a92d3cae0fe007fd7f4d80b68ee745
AMAL_SHA=1e71ddf93849c6a6ecf58b827c0692073d2dd7ee40196158068f7b29f422e87d
PREFIX=${SQLITE_PREFIX:-$HOME/.local/opt/sqlite-$VER}
# Bun's defines, in its order
DEFINES="-DSQLITE_ENABLE_COLUMN_METADATA=1 -DSQLITE_MAX_VARIABLE_NUMBER=250000 -DSQLITE_ENABLE_RTREE=1 -DSQLITE_ENABLE_FTS3=1 -DSQLITE_ENABLE_FTS3_PARENTHESIS=1 -DSQLITE_ENABLE_FTS5=1 -DSQLITE_ENABLE_JSON1=1 -DSQLITE_ENABLE_MATH_FUNCTIONS=1 -DSQLITE_ENABLE_UPDATE_DELETE_LIMIT=1 -DSQLITE_UDL_CAPABLE_PARSER=1 -DSQLITE_ENABLE_SESSION=1 -DSQLITE_ENABLE_PREUPDATE_HOOK=1 -DSQLITE_ENABLE_DBSTAT_VTAB=1 -DSQLITE_ENABLE_GEOPOLY=1 -DSQLITE_ENABLE_RBU=1 -DSQLITE_ENABLE_PERCENTILE=1"
UA="OpenGrid-research (+https://github.com/HeldvonKosmos/phprs)"
[ -f "$PREFIX/lib/libsqlite3.so.0" ] && [ -f "$PREFIX/src/sqlite3.c" ] && { echo "sqlite: $PREFIX exists"; exit 0; }
W=$(mktemp -d)
trap 'rm -rf "$W"' EXIT
curl -sSfL -A "$UA" -o "$W/sqlite3.c" "https://raw.githubusercontent.com/oven-sh/bun/$BUN_TAG/src/jsc/bindings/sqlite/sqlite3.c"
echo "$BUN_C_SHA  $W/sqlite3.c" | sha256sum -c - >/dev/null
curl -sSfL -A "$UA" -o "$W/amal.zip" "https://sqlite.org/2026/sqlite-amalgamation-3530400.zip"
echo "$AMAL_SHA  $W/amal.zip" | sha256sum -c - >/dev/null
(cd "$W" && unzip -q amal.zip)
mkdir -p "$PREFIX/src" "$PREFIX/include" "$PREFIX/lib/pkgconfig"
cp "$W/sqlite3.c" "$PREFIX/src/"
cp "$W/sqlite-amalgamation-3530400/sqlite3.h" "$W/sqlite-amalgamation-3530400/sqlite3ext.h" "$PREFIX/src/"
cp "$W/sqlite-amalgamation-3530400/sqlite3.h" "$W/sqlite-amalgamation-3530400/sqlite3ext.h" "$PREFIX/include/"
echo "-O2 $DEFINES" > "$PREFIX/src/CFLAGS"
# a shared library with the usual soname, as SQLite's own build installs it
taskset -c 0-7 cc -O2 -fPIC -shared $DEFINES -DSQLITE_THREADSAFE=1 -Wl,-soname,libsqlite3.so.0 \
  -o "$PREFIX/lib/libsqlite3.so.0.8.6" "$PREFIX/src/sqlite3.c" -lpthread -ldl -lm
ln -sfn libsqlite3.so.0.8.6 "$PREFIX/lib/libsqlite3.so.0"
ln -sfn libsqlite3.so.0.8.6 "$PREFIX/lib/libsqlite3.so"
cat > "$PREFIX/lib/pkgconfig/sqlite3.pc" <<EOF
prefix=$PREFIX
libdir=\${prefix}/lib
includedir=\${prefix}/include
Name: SQLite
Description: SQL database engine (the benchmark's build, BENCH3)
Version: $VER
Libs: -L\${libdir} -Wl,-rpath,\${libdir} -lsqlite3
Cflags: -I\${includedir}
EOF
echo "sqlite: installed $PREFIX"
