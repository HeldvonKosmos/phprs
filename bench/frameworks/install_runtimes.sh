#!/usr/bin/env bash
# Runtimes of the comparison frameworks (BENCH3), pinned and checked:
# Bun (AdonisJS), Ruby with YJIT (Rails), CPython (FastAPI), Java/Temurin
# (Spring Boot), the .NET SDK (ASP.NET Core), Erlang/OTP and Elixir
# (Phoenix), Go (Goravel), and FrankenPHP (Laravel) built from its sources
# against a ZTS PHP with the benchmark's SQLite, into ~/.local/opt/<name>.
# FrankenPHP's release binaries have SQLite 3.45.2 built in statically; the
# benchmark runs Bun's SQLite 3.53.4 on every side (Jonas, 28.10.), so
# FrankenPHP is built here: PHP with --enable-embed --enable-zts and
# pdo_sqlite linked against ~/.local/opt/sqlite-3.53.4, then FrankenPHP's
# Caddy module with `go build` against that libphp.
# CPython is built against the system's libsqlite3 (dynamically), so that it
# loads SQLite 3.53.4 through LD_LIBRARY_PATH like every other target.
#   bench/frameworks/install_runtimes.sh [bun|ruby|python|java|dotnet|erlang|elixir|go|air|php|frankenphp ...]
# Builds run on cores 0-7 with 2 jobs (the container's limit). Idempotent:
# an installed runtime with the right version is skipped.
set -euo pipefail
OPT=$HOME/.local/opt
DL=${RUNTIME_DL:-$(cd "$(dirname "$0")/../../.." && pwd)/target/runtime-dl}
UA="OpenGrid-research (+https://github.com/HeldvonKosmos/phprs)"
mkdir -p "$OPT" "$DL"

AIR=1.67.4
GO=1.27.2         GO_SHA=ecbadb99091a3f46e31f5f934b068b1864eafa7995211b39eaddf76996045fe5
PHP=8.5.11        PHP_SHA=d9be75c08e8c316f4c8f4194d8fbe1750a15f6a6d9d4e3fe72082abeeb800360
FRANKENPHP=1.13.0 FRANKENPHP_SHA=6bcb8fdb414899cfb74e712c552771ec31cbe14d07ec767ce6a3aa671b54ab36
SQLITE=$OPT/sqlite-3.53.4
BUN=1.4.3         BUN_SHA=3e730528d89775f03f87f05c06787c4df4099d0a37f3f252e7b829cb23816e18
RUBY=4.0.9        RUBY_SHA=c35350682ff050b79702d57ade4516e7336063f510155d19fb2fb5b3f3909511
PYTHON=3.15.2     PYTHON_SHA=8b5536d280142ecf6cdfe15d12a21ba1c65e300bbf7b28a8d0cdb849575797de
JDK=25.0.5+8      JDK_SHA=68982529869257d0ee6b824ec5aa270ff8b118bb438670da0b97ebdab178d7fb
OTP=29.1.2        OTP_SHA=c79966661e35fe4aff8ec003db63682e3b33b10b672220d51bf93c0a9244b049
ELIXIR=1.20.4     ELIXIR_SHA=7863c546cda13fecc949e562e326042451dacf8fd8698a36783cb71eeb223b46
# .NET 10 LTS: runtime 10.0.14 in SDK 10.0.403 (Microsoft publishes SHA-512)
DOTNET_SDK=10.0.403 DOTNET_SHA512=b7b3d8461cf98b07d2552ce9199702fa3ad010ac2671a5f8cd5136f7534e50f6855ba7bbb02b11494563f1d04156276a31cda1103b900e5eecd0d9473dc7b9c5

fetch() {  # fetch <url> <file> <sha256>
  local f=$DL/$2
  [ -f "$f" ] || curl -fsSL -A "$UA" -o "$f.part" "$1" && { [ -f "$f" ] || mv "$f.part" "$f"; }
  echo "$3  $f" | sha256sum -c --quiet - || { echo "checksum of $2 does not match" >&2; rm -f "$f"; exit 1; }
}
build() { taskset -c 0-7 "$@"; }
fetch512() {  # fetch512 <url> <file> <sha512>
  local f=$DL/$2
  [ -f "$f" ] || curl -fsSL -A "$UA" -o "$f.part" "$1" && { [ -f "$f" ] || mv "$f.part" "$f"; }
  echo "$3  $f" | sha512sum -c --quiet - || { echo "checksum of $2 does not match" >&2; rm -f "$f"; exit 1; }
}

bun() {
  [ -x "$OPT/bun-$BUN/bun" ] && return
  fetch "https://github.com/oven-sh/bun/releases/download/bun-v$BUN/bun-linux-x64.zip" "bun-$BUN-linux-x64.zip" "$BUN_SHA"
  rm -rf "$OPT/bun-$BUN" && mkdir -p "$OPT/bun-$BUN"
  (cd "$OPT/bun-$BUN" && unzip -q -j "$DL/bun-$BUN-linux-x64.zip")
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
dotnet() {
  [ -x "$OPT/dotnet-$DOTNET_SDK/dotnet" ] && return
  local f="dotnet-sdk-$DOTNET_SDK-linux-x64.tar.gz"
  fetch512 "https://builds.dotnet.microsoft.com/dotnet/Sdk/$DOTNET_SDK/$f" "$f" "$DOTNET_SHA512"
  rm -rf "$OPT/dotnet-$DOTNET_SDK" && mkdir -p "$OPT/dotnet-$DOTNET_SDK" && tar -xzf "$DL/$f" -C "$OPT/dotnet-$DOTNET_SDK"
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
  rm -rf "$OPT/elixir-$ELIXIR go-$GO php-$PHP-zts frankenphp-$FRANKENPHP" && mkdir -p "$OPT/elixir-$ELIXIR" && (cd "$OPT/elixir-$ELIXIR" && unzip -q "$DL/elixir-otp-${OTP%%.*}.zip")
}
go() {
  [ -x "$OPT/go-$GO/bin/go" ] && return
  fetch "https://go.dev/dl/go$GO.linux-amd64.tar.gz" "go$GO.linux-amd64.tar.gz" "$GO_SHA"
  rm -rf "$OPT/go-$GO" && mkdir -p "$OPT/go-$GO"
  tar -xzf "$DL/go$GO.linux-amd64.tar.gz" -C "$OPT/go-$GO" --strip-components=1
}
air() {
  # Goravel's development mode (the skeleton's .air.toml): rebuild and restart on change
  [ -x "$OPT/go-$GO/tools/air" ] && return
  go
  GOBIN=$OPT/go-$GO/tools GOTOOLCHAIN=local build "$OPT/go-$GO/bin/go" install "github.com/air-verse/air@v$AIR"
}
php() {
  # ZTS with the embed SAPI (libphp.so) and without Zend signals, as
  # FrankenPHP requires; the extensions a
  # Laravel app needs; pdo_sqlite and sqlite3 against the benchmark's SQLite;
  # opcache is part of PHP 8.5; no php.ini (the defaults, as in the release)
  [ -x "$OPT/php-$PHP-zts/bin/php-config" ] && return
  [ -f "$SQLITE/lib/libsqlite3.so.0" ] || bash "$(dirname "$0")/../framework/sqlite/build_sqlite.sh"
  fetch "https://www.php.net/distributions/php-$PHP.tar.xz" "php-$PHP.tar.xz" "$PHP_SHA"
  local src=$DL/php-$PHP && rm -rf "$src" && tar -xJf "$DL/php-$PHP.tar.xz" -C "$DL"
  (cd "$src" && PKG_CONFIG_PATH=$SQLITE/lib/pkgconfig LDFLAGS="-Wl,-rpath,$SQLITE/lib" build ./configure --prefix="$OPT/php-$PHP-zts" --enable-zts --enable-embed --disable-zend-signals \
      --disable-cgi --disable-phpdbg --with-pdo-sqlite --with-sqlite3 --enable-mbstring --with-openssl --with-zlib --with-curl \
      --enable-pcntl --enable-posix --enable-sockets --enable-bcmath >/dev/null \
    && build make -j2 >/dev/null && make install >/dev/null)
}
frankenphp() {
  [ -x "$OPT/frankenphp-$FRANKENPHP/frankenphp" ] && return
  go && php
  fetch "https://github.com/php/frankenphp/archive/refs/tags/v$FRANKENPHP.tar.gz" "frankenphp-$FRANKENPHP.tar.gz" "$FRANKENPHP_SHA"
  local src=$DL/frankenphp-$FRANKENPHP && rm -rf "$src" && tar -xzf "$DL/frankenphp-$FRANKENPHP.tar.gz" -C "$DL"
  local pc=$OPT/php-$PHP-zts/bin/php-config
  mkdir -p "$OPT/frankenphp-$FRANKENPHP"
  # as FrankenPHP's docs build it from source (caddy/frankenphp), without the
  # optional Caddy modules (badger, MySQL, PostgreSQL storage) and without the
  # file watcher (a C library; Octane reloads through octane:reload); the module
  # downloads are checked against go.sum; -a: the cgo part depends on the
  # PHP headers outside the module, which the build cache does not track
  (cd "$src/caddy/frankenphp" && CGO_ENABLED=1 GOTOOLCHAIN=local GOFLAGS=-trimpath \
    CGO_CFLAGS="$("$pc" --includes)" CGO_LDFLAGS="$("$pc" --ldflags) -L$SQLITE/lib -Wl,-rpath,$SQLITE/lib -L$OPT/php-$PHP-zts/lib -Wl,-rpath,$OPT/php-$PHP-zts/lib -lphp $("$pc" --libs)" \
    build "$OPT/go-$GO/bin/go" build -a -tags=nobadger,nomysql,nopgx,nowatcher -ldflags "-X 'github.com/caddyserver/caddy/v2.CustomVersion=FrankenPHP v$FRANKENPHP PHP $PHP Caddy'" -o "$OPT/frankenphp-$FRANKENPHP/frankenphp")
}

for r in "${@:-bun ruby python java dotnet erlang elixir go air php frankenphp}"; do
  for x in $r; do echo "== $x"; "$x"; done
done
# the PATH entries for the benchmark (bench/frameworks/env.sh sources this list)
echo "runtimes in $OPT: bun-$BUN ruby-$RUBY python-$PYTHON jdk-$JDK dotnet-$DOTNET_SDK otp-$OTP elixir-$ELIXIR go-$GO php-$PHP-zts frankenphp-$FRANKENPHP"
