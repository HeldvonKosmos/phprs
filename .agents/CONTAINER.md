# Container: what was installed into the running container, or is still to come

Jonas only rebuilds the image when the container has been stopped. Every installation is therefore recorded twice: as the command
for the running container (here) and in `.devpod/Containerfile` (for the next rebuild).

| What | Command in the running container | In the Containerfile | State |
|---|---|---|---|
| Rust nightly with Miri (runtime functions of the JIT) | `rustup toolchain install nightly --profile minimal --component miri,rust-src && cargo +nightly miri setup` | yes | **installed** 2026-10-26 |
| sccache, `jobs = 2` in `~/.cargo/config.toml` | (was already there) | yes (since 2026-10-27) | present |
| SQLite 3.45.2 with FrankenPHP's compile options (core benchmark, D23) | `phprs/bench/framework/sqlite/build_sqlite.sh` (→ `~/.local/opt/sqlite-3.45.2`) | no (built on demand by `bm2.py`) | **installed** 2026-10-27 |
| FrankenPHP v1.13.4 (Octane server) | `phprs/bench/framework/laravel/setup.sh` (downloads into the Laravel app, checksum) | no (part of the app) | **installed** 2026-10-27 |
| `valgrind` (Callgrind, profiles in PERF4/JIT; since 2026-10-25 only installed by hand) and `valgrind-gdb` (`vgdb`, needed for `callgrind_control`, i.e. for profiles of a running server: `bench/framework/profile_core.py`; pulls in `gdb`) | `sudo dnf -y install valgrind valgrind-gdb` | yes (since 2026-10-27) | **installed** (valgrind 2026-10-25, valgrind-gdb 2026-10-27) |
| **PostgreSQL removed** (D24, Jonas 2026-10-27: both stacks run on SQLite): `postgresql-server`, `postgresql`, `postgresql-private-libs`, `php-pgsql`, `libpq`, `libpq-devel` uninstalled, `.pgdata` (696 MB of benchmark data) deleted; in the Containerfile `postgresql17`, `postgresql17-server`, `php-pgsql`, `libpq-devel` dropped | `pg_ctl -D .pgdata -m fast stop && sudo dnf -y remove postgresql-server postgresql postgresql-private-libs php-pgsql libpq-devel libpq` | yes (removed) | **removed** 2026-10-27 |
| Build dependencies of the comparison runtimes (BENCH3): `ncurses-devel libyaml-devel libffi-devel zlib-ng-compat-devel readline-devel bzip2-devel xz-devel libuuid-devel gdbm-devel unzip autoconf`, for PHP-ZTS/FrankenPHP additionally `oniguruma-devel libxml2-devel libcurl-devel`, for Rails `jemalloc` (LD_PRELOAD) | `sudo dnf -y install …` (list as on the left) | yes (since 2026-10-28) | **installed** 2026-10-28 |
| Runtimes for BENCH3: Bun 1.4.3, Ruby 4.0.9 (YJIT), Python 3.15.2 (against the system SQLite), Temurin JDK 25.0.5+8, .NET 10.0.403, Erlang/OTP 29.1.2, Elixir 1.20.4, Go 1.27.2 (+ air 1.67.4), PHP 8.5.11 ZTS (embed) and FrankenPHP 1.13.0 from source, SQLite 3.53.4 (Bun's amalgamation and options) into `~/.local/opt/` | `phprs/bench/frameworks/install_runtimes.sh` (pinned, checksums) | note in the Containerfile (run the script after mounting the repository) | in progress 2026-10-28 |
| `diffutils` (`cmp`, `diff`; were missing, SQLite's configure warned; `tools/release.sh` needs `cmp`) | `sudo dnf -y install diffutils` | yes | **installed** 2026-10-27 |
| vite-plus 1.4.0, Playwright 1.64.1 with headless Chromium | `sudo dnf -y install chromium-headless && npm config set prefix ~/.local && npm install -g vite-plus@1.4.0 playwright@1.64.1 && playwright install chromium-headless-shell` | yes | **not yet** (UI1 deferred, D22) |
