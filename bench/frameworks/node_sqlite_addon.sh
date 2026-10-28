#!/usr/bin/env bash
# better-sqlite3 of a Node comparison app against the benchmark's SQLite
# (BENCH3; AdonisJS, NestJS, Next.js): compiled from the
# amalgamation of the same SQLite 3.45.2 tarball, with the same compile
# options as the shared library the other sides load (bench/framework/sqlite/
# build_sqlite.sh writes both), instead of the SQLite it ships. The prebuilt
# Linux binaries are removed, so the loader takes the built addon.
#   bench/frameworks/node_sqlite_addon.sh <app dir>
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
cd "${1:?app directory}"
export PATH=$HOME/.local/opt/node-24.22.1/bin:$PATH
P=${SQLITE_PREFIX:-$HOME/.local/opt/sqlite-3.45.2}
[ -f "$P/src/sqlite3.c" ] || "$HERE/../framework/sqlite/build_sqlite.sh"
# the amalgamation with the library's -D options as #defines (better-sqlite3
# adds only SQLITE_ENABLE_COLUMN_METADATA to a custom amalgamation)
mkdir -p "$P/src-defines"
{ for d in $(tr ' ' '\n' < "$P/src/CFLAGS" | grep '^-D' | sed 's/^-D//'); do
    k=${d%%=*}; v=${d#*=}; [ "$k" = "$d" ] && v=1; echo "#define $k $v"; done
  cat "$P/src/sqlite3.c"; } > "$P/src-defines/sqlite3.c"
cp "$P/src/sqlite3.h" "$P/src/sqlite3ext.h" "$P/src-defines/"
cd node_modules/better-sqlite3
rm -rf build prebuilds/linux-x64.node prebuilds/linux-arm64.node prebuilds/linuxmusl-*.node
taskset -c 0-7 npx --yes node-gyp@13.1.0 rebuild --release --jobs=2 --force_build=1 --sqlite3="$P/src-defines" >/dev/null
node -e 'const D=require("better-sqlite3"); const d=new D(":memory:"); console.log("better-sqlite3 uses SQLite " + d.prepare("select sqlite_version() v").get().v)'
