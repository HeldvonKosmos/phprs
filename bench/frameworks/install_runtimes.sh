#!/usr/bin/env bash
# Runtimes of the comparison frameworks (BENCH3), pinned and checked:
# Node (AdonisJS), Ruby with YJIT (Rails), CPython (Django), Java/Temurin
# (Spring Boot), Erlang/OTP and Elixir (Phoenix), into ~/.local/opt/<name>.
# CPython is built against the system's libsqlite3 (dynamically), so that it
# loads SQLite 3.45.2 through LD_LIBRARY_PATH like every other target.
#   bench/frameworks/install_runtimes.sh [node|ruby|python|java|erlang|elixir ...]
# Builds run on cores 0-7 with 2 jobs (the container's limit). Idempotent:
# an installed runtime with the right version is skipped.
set -euo pipefail
OPT=$HOME/.local/opt
DL=${RUNTIME_DL:-$(cd "$(dirname "$0")/../../.." && pwd)/target/runtime-dl}
UA="OpenGrid-research (+https://github.com/HeldvonKosmos/phprs)"
mkdir -p "$OPT" "$DL"

NODE=24.22.1      NODE_SHA=6c172a9ab6718f7b801fb5a7bd486af2ee5e32758086a04b84fcce2aa77095e6
RUBY=4.0.9        RUBY_SHA=c35350682ff050b79702d57ade4516e7336063f510155d19fb2fb5b3f3909511
PYTHON=3.15.2     PYTHON_SHA=8b5536d280142ecf6cdfe15d12a21ba1c65e300bbf7b28a8d0cdb849575797de
JDK=25.0.5+8      JDK_SHA=68982529869257d0ee6b824ec5aa270ff8b118bb438670da0b97ebdab178d7fb
OTP=29.1.2        OTP_SHA=c79966661e35fe4aff8ec003db63682e3b33b10b672220d51bf93c0a9244b049
ELIXIR=1.20.4     ELIXIR_SHA=7863c546cda13fecc949e562e326042451dacf8fd8698a36783cb71eeb223b46

fetch() {  # fetch <url> <file> <sha256>
  local f=$DL/$2
  [ -f "$f" ] || curl -fsSL -A "$UA" -o "$f.part" "$1" && { [ -f "$f" ] || mv "$f.part" "$f"; }
  echo "$3  $f" | sha256sum -c --quiet - || { echo "checksum of $2 does not match" >&2; rm -f "$f"; exit 1; }
}
build() { taskset -c 0-7 "$@"; }

node() {
  [ -x "$OPT/node-$NODE/bin/node" ] && return
  fetch "https://nodejs.org/dist/v$NODE/node-v$NODE-linux-x64.tar.xz" "node-v$NODE-linux-x64.tar.xz" "$NODE_SHA"
  rm -rf "$OPT/node-$NODE" && mkdir -p "$OPT/node-$NODE"
  tar -xJf "$DL/node-v$NODE-linux-x64.tar.xz" -C "$OPT/node-$NODE" --strip-components=1
}
ruby() {
  [ -x "$OPT/ruby-$RUBY/bin/ruby" ] && return
  fetch "https://cache.ruby-lang.org/pub/ruby/${RUBY%.*}/ruby-$RUBY.tar.gz" "ruby-$RUBY.tar.gz" "$RUBY_SHA"
  local src=$DL/ruby-$RUBY && rm -rf "$src" && tar -xzf "$DL/ruby-$RUBY.tar.gz" -C "$DL"
  (cd "$src" && build ./configure --prefix="$OPT/ruby-$RUBY" --enable-yjit --disable-install-doc >/dev/null && build make -j2 >/dev/null && make install >/dev/null)
}
python() {
  [ -x "$OPT/python-$PYTHON/bin/python3" ] && return
  fetch "https://www.python.org/ftp/python/$PYTHON/Python-$PYTHON.tar.xz" "Python-$PYTHON.tar.xz" "$PYTHON_SHA"
  local src=$DL/Python-$PYTHON && rm -rf "$src" && tar -xJf "$DL/Python-$PYTHON.tar.xz" -C "$DL"
  # the official release optimisations; the system's libsqlite3 (dynamic)
  (cd "$src" && build ./configure --prefix="$OPT/python-$PYTHON" --enable-optimizations --with-lto --enable-loadable-sqlite-extensions >/dev/null \
     && build make -j2 >/dev/null && make install >/dev/null)
}
java() {
  [ -x "$OPT/jdk-$JDK/bin/java" ] && return
  local f="OpenJDK25U-jdk_x64_linux_hotspot_${JDK/+/_}.tar.gz"
  fetch "https://github.com/adoptium/temurin25-binaries/releases/download/jdk-${JDK/+/%2B}/$f" "$f" "$JDK_SHA"
  rm -rf "$OPT/jdk-$JDK" && mkdir -p "$OPT/jdk-$JDK" && tar -xzf "$DL/$f" -C "$OPT/jdk-$JDK" --strip-components=1
}
erlang() {
  [ -x "$OPT/otp-$OTP/bin/erl" ] && return
  fetch "https://github.com/erlang/otp/releases/download/OTP-$OTP/otp_src_$OTP.tar.gz" "otp_src_$OTP.tar.gz" "$OTP_SHA"
  local src=$DL/otp_src_$OTP && rm -rf "$src" && tar -xzf "$DL/otp_src_$OTP.tar.gz" -C "$DL"
  (cd "$src" && build ./configure --prefix="$OPT/otp-$OTP" --without-javac --without-wx --without-odbc >/dev/null && build make -j2 >/dev/null && make install >/dev/null)
}
elixir() {
  [ -x "$OPT/elixir-$ELIXIR/bin/elixir" ] && return
  fetch "https://github.com/elixir-lang/elixir/releases/download/v$ELIXIR/elixir-otp-${OTP%%.*}.zip" "elixir-otp-${OTP%%.*}.zip" "$ELIXIR_SHA"
  rm -rf "$OPT/elixir-$ELIXIR" && mkdir -p "$OPT/elixir-$ELIXIR" && (cd "$OPT/elixir-$ELIXIR" && unzip -q "$DL/elixir-otp-${OTP%%.*}.zip")
}

for r in "${@:-node ruby python java erlang elixir}"; do
  for x in $r; do echo "== $x"; "$x"; done
done
# the PATH entries for the benchmark (bench/frameworks/env.sh sources this list)
echo "runtimes in $OPT: node-$NODE ruby-$RUBY python-$PYTHON jdk-$JDK otp-$OTP elixir-$ELIXIR"
