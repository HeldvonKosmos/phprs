#!/usr/bin/env bash
# sqlite-jdbc's JNI library against the benchmark's SQLite (BENCH3). The
# driver loads its own libsqlitejdbc.so with SQLite compiled in; this builds
# that JNI part (NativeDB.c of the driver's tag 3.53.4.0, the version made
# for SQLite 3.53.4) linked dynamically against ~/.local/opt/sqlite-3.53.4,
# so the JVM runs the same libsqlite3.so.0 as PHP, Python and Ruby. The JVM
# loads it through -Dorg.sqlite.lib.path / -Dorg.sqlite.lib.name (serve.sh).
#   bench/spring-app/scripts/build_sqlite_jni.sh  -> bench/spring-app/native/libsqlitejdbc.so
set -euo pipefail
cd "$(dirname "$0")/.."
OPT=$HOME/.local/opt
JAVA_HOME=$OPT/jdk-25.0.5+8
SQLITE=$OPT/sqlite-3.53.4
DL=${RUNTIME_DL:-$(cd ../../.. && pwd)/target/runtime-dl}
UA="OpenGrid-research (+https://github.com/HeldvonKosmos/phprs)"
VER=3.53.4.0 SRC_SHA=9dded7305b4f3bab596d6a40eaab0cf0d3afad60006eb80073492103b06b081c
SLF4J=1.7.36 SLF4J_SHA=d3ef575e3e4979678dc01bf1dcce51021493b4d11fb7f1be8ad982877c16a1c0
mkdir -p "$DL" native
fetch() { [ -f "$DL/$2" ] || curl -fsSL -A "$UA" -o "$DL/$2" "$1"; echo "$3  $DL/$2" | sha256sum -c --quiet -; }
fetch "https://github.com/xerial/sqlite-jdbc/archive/refs/tags/$VER.tar.gz" "sqlite-jdbc-$VER.tar.gz" "$SRC_SHA"
fetch "https://repo1.maven.org/maven2/org/slf4j/slf4j-api/$SLF4J/slf4j-api-$SLF4J.jar" "slf4j-api-$SLF4J.jar" "$SLF4J_SHA"
W=$(mktemp -d)
trap 'rm -rf "$W"' EXIT
tar -xzf "$DL/sqlite-jdbc-$VER.tar.gz" -C "$W"
S=$W/sqlite-jdbc-$VER/src/main/java
# the JNI header, as the driver's Makefile makes it
"$JAVA_HOME/bin/javac" -cp "$DL/slf4j-api-$SLF4J.jar" -d "$W/classes" -sourcepath "$S" -h "$W/h" "$S/org/sqlite/core/NativeDB.java"
mv "$W/h/org_sqlite_core_NativeDB.h" "$W/h/NativeDB.h"
taskset -c 0-7 gcc -O2 -fPIC -shared -I"$JAVA_HOME/include" -I"$JAVA_HOME/include/linux" -I"$W/h" -I"$SQLITE/include" \
  "$S/org/sqlite/core/NativeDB.c" -L"$SQLITE/lib" -Wl,-rpath,"$SQLITE/lib" -lsqlite3 -o native/libsqlitejdbc.so
echo "built native/libsqlitejdbc.so against $(ldd native/libsqlitejdbc.so | grep libsqlite3)"
