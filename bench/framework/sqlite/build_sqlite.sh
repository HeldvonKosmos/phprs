#!/usr/bin/env bash
# SQLite of the framework-core benchmark (D23): the version FrankenPHP's
# static build ships (3.45.2) with the compile options of that build
# (PRAGMA compile_options compared: identical apart from the compiler), as a
# shared library. OpenGrid, PHP-FPM and RoadRunner load it through
# LD_LIBRARY_PATH (bm2.py), FrankenPHP has the same version built in, so all
# core targets run the same SQLite.
#   bench/framework/sqlite/build_sqlite.sh   -> ~/.local/opt/sqlite-3.45.2/lib
set -euo pipefail
VER=3450200
SHA=bc9067442eedf3dd39989b5c5cfbfff37ae66cc9c99274e0c3052dc4d4a8f6ae
PREFIX=${SQLITE_PREFIX:-$HOME/.local/opt/sqlite-3.45.2}
[ -f "$PREFIX/lib/libsqlite3.so.0" ] && { echo "sqlite: $PREFIX exists"; exit 0; }
W=$(mktemp -d)
trap 'rm -rf "$W"' EXIT
curl -sSfL -A "OpenGrid-research (+https://github.com/HeldvonKosmos/phprs)" -o "$W/src.tgz" "https://www.sqlite.org/2024/sqlite-autoconf-$VER.tar.gz"
echo "$SHA  $W/src.tgz" | sha256sum -c - >/dev/null
tar xzf "$W/src.tgz" -C "$W"
cd "$W/sqlite-autoconf-$VER"
CFLAGS="-O2 -DSQLITE_ENABLE_COLUMN_METADATA -DSQLITE_ENABLE_FTS3 -DSQLITE_ENABLE_FTS4 -DSQLITE_ENABLE_FTS5 -DSQLITE_ENABLE_GEOPOLY -DSQLITE_ENABLE_MATH_FUNCTIONS -DSQLITE_ENABLE_RTREE -DSQLITE_THREADSAFE=1" \
  ./configure --prefix="$PREFIX" --disable-static --enable-shared >/dev/null
make -j2 >/dev/null
make install >/dev/null
echo "sqlite: installed $PREFIX"
