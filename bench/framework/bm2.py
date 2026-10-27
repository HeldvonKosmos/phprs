#!/usr/bin/env python3
"""BM2: framework benchmark over HTTP, OpenGrid against the Laravel comparison
application (LV1), method D11 (BENCHMARKS.md section 4).

Targets (each started fresh, data created over the API, no seeds):
  opengrid-server    OpenGrid (examples/test-app) against a SurrealDB server (ws://)
  opengrid-embedded  OpenGrid with the embedded surrealkv database
  opengrid-embedded-pgo  the same with the PGO binary (--opengrid-pgo, PGO1;
                     trained on api.sh, never on this load)
  laravel-fpm        Laravel + Sanctum, nginx + PHP-FPM (opcache on, JIT off)
  laravel-octane     Laravel + Sanctum, Octane on RoadRunner
  laravel-fpm-pgsql, laravel-octane-pgsql
                     the same on PostgreSQL (LV2, "Vergleich der Standard-Setups"):
                     the in-container server of pg.sh (started when needed;
                     another server only through PGHOST), a fresh database
                     per run, connection only through PG* variables
  <target>-core      framework core (D23): opengrid-embedded-core,
                     opengrid-embedded-pgo-core, laravel-octane-core,
                     laravel-fpm-core run the same raw SQL on a fresh SQLite
                     database (bench/framework/sqlite/), endpoints
                     /api/bench/sql/…, the token check in both apps the same
  <opengrid target>-touchN
                     any OpenGrid target with auth.token_touch_seconds = N:
                     touch1 writes the token's last use at most once a second,
                     as Sanctum does in effect (Eloquent saves last_used_at
                     only when it changed, to the second; querylog.py, D11
                     2026-10-27); touch0 writes on every request, stricter
                     than Laravel. E.g. opengrid-embedded-touch1.

Endpoints: GET /health, GET /api/notes?filter[done]=false&sort=priority:desc,
GET /api/notes/{id}, POST /api/notes (all with a bearer token), POST /api/login
(bcrypt cost 10 on both sides). Load generator oha: per endpoint and
concurrency a warm-up run, then the measured run; requests/s, p50, p99.
Server, database server and load generator are pinned to separate cores; the
same number of workers on both sides.

usage: bm2.py [--targets …] [--workers 4] [--conc 1,16,64] [--duration 10] [--warmup 3] [--notes 200]
"""
import argparse, datetime, json, os, re, secrets, shutil, signal, socket, subprocess, sys, tempfile, time, urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.abspath(os.path.join(HERE, "../.."))
WS = os.path.abspath(os.path.join(REPO, ".."))
TEST_APP = os.path.join(REPO, "examples/test-app")
SERVE = os.path.join(HERE, "laravel/serve.sh")
LARAVEL_APP = os.environ.get("LARAVEL_APP", os.path.join(WS, "bench/laravel-app"))
ALL_TARGETS = ["opengrid-server", "opengrid-embedded", "laravel-fpm", "laravel-octane"]
ENDPOINTS = ["health", "list", "show", "create", "login"]


def free_port():
    s = socket.socket()
    s.bind(("127.0.0.1", 0))
    p = s.getsockname()[1]
    s.close()
    return p


def http(method, url, body=None, token=None):
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(url, data=data, method=method)
    if data is not None:
        req.add_header("Content-Type", "application/json")
    if token:
        req.add_header("Authorization", "Bearer " + token)
    try:
        with urllib.request.urlopen(req, timeout=30) as r:
            raw = r.read()
            return r.status, (json.loads(raw) if raw else None)
    except urllib.error.HTTPError as e:
        return e.code, e.read().decode(errors="replace")


def wait_http(url, proc, what, timeout=60):
    end = time.time() + timeout
    while time.time() < end:
        if proc.poll() is not None:
            sys.exit(f"{what} exited early (code {proc.returncode})")
        try:
            with urllib.request.urlopen(url, timeout=2) as r:
                if r.status == 200:
                    return
        except Exception:
            pass
        time.sleep(0.2)
    sys.exit(f"{what} did not answer on {url}")


def start(cmd, cores, env, log, cwd=None):
    return subprocess.Popen(["taskset", "-c", cores] + cmd, env=env, cwd=cwd, stdout=open(log, "ab"), stderr=subprocess.STDOUT, start_new_session=True)


def stop(proc):
    if proc and proc.poll() is None:
        os.killpg(proc.pid, signal.SIGTERM)
        try:
            proc.wait(15)
        except subprocess.TimeoutExpired:
            os.killpg(proc.pid, signal.SIGKILL)
            proc.wait()


def oha(url, conc, duration, cores, method="GET", body=None, token=None):
    cmd = ["taskset", "-c", cores, "oha", "--no-tui", "--output-format", "json", "-z", f"{duration}s", "-c", str(conc), "-m", method]
    if token:
        cmd += ["-H", f"Authorization: Bearer {token}"]
    if body is not None:
        cmd += ["-T", "application/json", "-d", json.dumps(body)]
    out = subprocess.run(cmd + [url], capture_output=True, text=True)
    if out.returncode != 0:
        sys.exit(f"oha failed: {out.stderr[-500:]}")
    return json.loads(out.stdout)


def patch_config(app, workers, touch=None):
    p = os.path.join(app, "config/development.yaml")
    s = open(p).read()
    s = s.replace("level: info", "level: warn").replace("workers: 2", f"workers: {workers}\n    debug: false\n    watch: false")
    if touch is not None:
        assert "  auth:\n" in s, "settings.auth missing in the test app's config"
        s = s.replace("  auth:\n", f"  auth:\n    token_touch_seconds: {touch}\n", 1)
    open(p, "w").write(s)


PG_TCP = "127.0.0.1"
SQLITE_LIB = os.path.join(os.path.expanduser("~"), ".local/opt/sqlite-3.45.2/lib")
PG_SOCKET = os.path.join(WS, ".pgdata")
# --pg socket (default): Laravel's best case, no network stack; --pg tcp: 127.0.0.1:5432
PG_MODE = {"mode": "socket"}


def pg_env():
    # the in-container server of pg.sh, by default over its Unix socket
    # (Laravel's best case, DECISIONS 27.10.), with --pg tcp over 127.0.0.1;
    # another server only through an explicit PGHOST; credentials only
    # through the environment
    user = os.environ.get("PGUSER", os.environ.get("USER", "dev"))
    local = PG_TCP if PG_MODE["mode"] == "tcp" else PG_SOCKET
    return {"PGHOST": os.environ.get("PGHOST", local), "PGPORT": os.environ.get("PGPORT", "5432"), "PGUSER": user, "PGPASSWORD": os.environ.get("PGPASSWORD", "")}


class Target:
    def __init__(self, name, a, work):
        self.name, self.a, self.work = name, a, work
        self.port = free_port()
        self.base = f"http://127.0.0.1:{self.port}"
        self.procs = []
        self.env = dict(os.environ)
        for k in ("OPENGRID_DB_URL", "SURREAL_DB_HOST", "GRD_JIT", "OPENGRID_JIT"):
            self.env.pop(k, None)
        self.pg_db = None
        # D23 (DBR1): the framework core with the same raw SQL on SQLite
        self.core = name.endswith("-core")

    def surreal_server(self):
        port = free_port()
        pw = secrets.token_hex(16)
        env = dict(self.env, SURREAL_USER="root", SURREAL_PASS=pw, SURREAL_LOG="warn")
        p = start(["surreal", "start", "--bind", f"127.0.0.1:{port}", f"surrealkv://{self.work}/surreal"], self.a.db_cores, env, f"{self.work}/surreal.log")
        self.procs.append(p)
        wait_http(f"http://127.0.0.1:{port}/health", p, "surreal")
        # credentials only through the environment, never on a command line
        self.env.update(SURREAL_DB_HOST="127.0.0.1", SURREAL_DB_PORT=str(port), SURREAL_DB_NAMESPACE="bench", SURREAL_DB_DATABASE="bench",
                        SURREAL_DB_USERNAME="root", SURREAL_DB_PASSWORD=pw)

    def launch(self):
        w = self.a.workers
        if self.core:
            # a fresh SQLite database of the same schema and data for both apps
            db = os.path.join(self.work, "bench.sqlite")
            subprocess.run(["python3", os.path.join(HERE, "sqlite", "seed.py"), db, str(self.a.notes)], check=True, capture_output=True)
            self.env.update(OPENGRID_DB_BENCH_SQLITE_URL=f"sqlite://{db}", BENCH_SQLITE_DATABASE=db)
            # the SQLite FrankenPHP ships (3.45.2, same compile options) for
            # OpenGrid, PHP-FPM and RoadRunner too (sqlite/build_sqlite.sh)
            self.env["LD_LIBRARY_PATH"] = SQLITE_LIB + (":" + self.env["LD_LIBRARY_PATH"] if self.env.get("LD_LIBRARY_PATH") else "")
        if self.name.startswith("opengrid"):
            app = os.path.join(self.work, "app")
            shutil.copytree(TEST_APP, app, ignore=shutil.ignore_patterns(".opengrid", "tests"))
            m = re.search(r"-touch(\d+)$", self.name)
            patch_config(app, w, int(m.group(1)) if m else None)
            if self.name.startswith("opengrid-server"):
                self.surreal_server()
            else:
                self.env["OPENGRID_DB_URL"] = f"surrealkv://{self.work}/db"
            binary = self.a.opengrid_pgo if "-pgo" in self.name else self.a.opengrid
            r = subprocess.run([binary, "task", "migrate_push"], cwd=app, env=self.env, capture_output=True, text=True)
            if r.returncode != 0:
                sys.exit(f"migrate_push failed: {r.stdout[-800:]}{r.stderr[-800:]}")
            p = start([binary, "start", "--no-banner", "--binding", "127.0.0.1", "--port", str(self.port)], self.a.app_cores, self.env, f"{self.work}/server.log", cwd=app)
        elif self.name.endswith("-pgsql"):
            pg = pg_env()
            self.pg_db = f"bm2_{os.getpid()}_{secrets.token_hex(4)}"
            subprocess.run(["psql", "-q", "-d", "postgres", "-c", f"CREATE DATABASE {self.pg_db}"], env=dict(self.env, **pg), check=True)
            self.env.update(pg, DB_CONNECTION="pgsql", DB_HOST=pg["PGHOST"], DB_PORT=pg["PGPORT"], DB_DATABASE=self.pg_db, DB_USERNAME=pg["PGUSER"], DB_PASSWORD=pg["PGPASSWORD"])
        else:
            db = os.path.join(self.work, "db.sqlite")
            open(db, "w").close()
            self.env.update(DB_CONNECTION="sqlite", DB_DATABASE=db)
        if self.name.startswith("laravel"):
            subprocess.run(["php", "artisan", "config:clear"], cwd=LARAVEL_APP, env=self.env, capture_output=True)
            r = subprocess.run(["php", "artisan", "migrate", "--force"], cwd=LARAVEL_APP, env=self.env, capture_output=True, text=True)
            if r.returncode != 0:
                sys.exit(f"artisan migrate failed: {r.stdout[-800:]}{r.stderr[-800:]}")
            mode = self.name.split("-")[1]
            p = start([SERVE, mode, str(self.port), os.path.join(self.work, "run"), str(w)], self.a.app_cores, self.env, f"{self.work}/server.log")
        self.procs.append(p)
        wait_http(self.base + "/health", p, self.name, timeout=120)

    def seed(self):
        if self.core:
            st, body = http("POST", self.base + "/api/bench/sql/login", {"email": "bench@example.com", "password": "secret-123"})
            assert st == 200, (st, body)
            self.token = body["token"]
            self.note_id = str(self.a.notes // 2 + 1)
            return
        user = {"name": "Bench", "email": "bench@example.com", "password": "secret-123", "password_confirmation": "secret-123"}
        st, _ = http("POST", self.base + "/api/register", user)
        assert st == 201, st
        st, body = http("POST", self.base + "/api/login", {"email": user["email"], "password": user["password"]})
        assert st == 200, (st, body)
        self.token = body["token"]
        for i in range(self.a.notes):
            st, body = http("POST", self.base + "/api/notes", {"title": f"Note {i:04d}", "body": "x" * 40, "done": i % 2 == 0, "priority": i % 5 + 1}, self.token)
            assert st == 201, (st, body)
            if i == self.a.notes // 2:
                self.note_id = body["id"].split(":", 1)[1]

    def endpoint(self, e):
        b, t = self.base, self.token
        if self.core:
            c = b + "/api/bench/sql"
            return {
                "health": (c + "/health", "GET", None, None),
                "list": (c + "/notes?done=false&per_page=15", "GET", None, t),
                "show": (c + f"/notes/{self.note_id}", "GET", None, t),
                "create": (c + "/notes", "POST", {"title": "Bench", "body": "created by bm2", "priority": 2}, t),
                "login": (c + "/login", "POST", {"email": "bench@example.com", "password": "secret-123"}, None),
            }[e]
        return {
            "health": (b + "/health", "GET", None, None),
            "list": (b + "/api/notes?filter%5Bdone%5D=false&sort=priority:desc&per_page=15", "GET", None, t),
            "show": (b + f"/api/notes/{self.note_id}", "GET", None, t),
            "create": (b + "/api/notes", "POST", {"title": "Bench", "body": "created by bm2", "priority": 2}, t),
            "login": (b + "/api/login", "POST", {"email": "bench@example.com", "password": "secret-123"}, None),
        }[e]

    def close(self):
        for p in reversed(self.procs):
            stop(p)
        if self.pg_db:
            subprocess.run(["psql", "-q", "-d", "postgres", "-c", f"DROP DATABASE IF EXISTS {self.pg_db} WITH (FORCE)"], env=dict(self.env, **pg_env()), capture_output=True)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--targets", default=",".join(ALL_TARGETS))
    ap.add_argument("--endpoints", default=",".join(ENDPOINTS))
    ap.add_argument("--workers", type=int, default=4)
    ap.add_argument("--conc", default="1,16,64")
    ap.add_argument("--duration", type=int, default=10)
    ap.add_argument("--warmup", type=int, default=3)
    ap.add_argument("--notes", type=int, default=200)
    ap.add_argument("--app-cores", default="8-11")
    ap.add_argument("--db-cores", default="4-7")
    ap.add_argument("--load-cores", default="0-3")
    ap.add_argument("--opengrid", default=os.path.join(WS, "target/profiling/opengrid"))
    ap.add_argument("--opengrid-pgo", default=os.path.join(WS, "target-pgo/opengrid-pgo"))
    ap.add_argument("--json", help="write the raw results here")
    ap.add_argument("--pg", choices=["socket", "tcp"], default="socket", help="how Laravel reaches the in-container PostgreSQL")
    ap.add_argument("--label", default="", help="heading of the result, e.g. 'Vergleich der Standard-Setups'")
    a = ap.parse_args()
    PG_MODE["mode"] = a.pg
    a.opengrid = os.path.abspath(a.opengrid)
    a.opengrid_pgo = os.path.abspath(a.opengrid_pgo)
    sh = lambda c, cwd=None: subprocess.run(c, capture_output=True, text=True, cwd=cwd).stdout.strip()
    laravel = sh(["php", "artisan", "--version"], LARAVEL_APP) if os.path.isdir(LARAVEL_APP) else "–"
    platform_db = os.path.isdir(os.path.join(LARAVEL_APP, "app/Database"))
    if any(t.endswith("-pgsql") for t in a.targets.split(",")) and "PGHOST" not in os.environ:
        # the in-container server (data on the volume); start it when needed
        subprocess.run([os.path.join(HERE, "pg.sh"), "start"], check=True, capture_output=True)
    if a.label:
        print(f"### {a.label}\n")
    pg_version = subprocess.run(["psql", "-At", "-d", "postgres", "-c", "SHOW server_version"], capture_output=True, text=True, env=dict(os.environ, **pg_env())).stdout.strip() if any(t.endswith("-pgsql") for t in a.targets.split(",")) else ""
    if pg_version:
        where = {PG_TCP: "im Workbench-Container (pg.sh), TCP 127.0.0.1:5432, Kerne der DB-Server", PG_SOCKET: "im Workbench-Container (pg.sh), Unix-Socket, Kerne der DB-Server"}.get(pg_env()["PGHOST"], f"Server {pg_env()['PGHOST']}, nicht gepinnt")
        pg_dur = subprocess.run(["psql", "-At", "-d", "postgres", "-c", "select string_agg(name || '=' || setting, ', ' order by name) from pg_settings where name in ('fsync','synchronous_commit','full_page_writes','wal_sync_method')"], capture_output=True, text=True, env=dict(os.environ, **pg_env())).stdout.strip()
        print(f"PostgreSQL {pg_version} ({where}; {pg_dur}) · ", end="")
    if any(t.endswith("-core") for t in a.targets.split(",")):
        if not os.path.exists(os.path.join(SQLITE_LIB, "libsqlite3.so.0")):
            subprocess.run([os.path.join(HERE, "sqlite", "build_sqlite.sh")], check=True)
        lib_env = dict(os.environ, LD_LIBRARY_PATH=SQLITE_LIB)
        version_php = "echo (new PDO('sqlite::memory:'))->query('select sqlite_version()')->fetchColumn();"
        php_sqlite = subprocess.run(["php", "-r", version_php], capture_output=True, text=True, env=lib_env).stdout.strip()
        og_lib = next((l.split("=>")[1].split("(")[0].strip() for l in subprocess.run(["ldd", a.opengrid], capture_output=True, text=True, env=lib_env).stdout.splitlines() if "libsqlite3" in l), "?")
        fp = os.path.join(LARAVEL_APP, "frankenphp")
        fp_sqlite = "–"
        if os.path.exists(fp):
            with tempfile.NamedTemporaryFile("w", suffix=".php", delete=False) as f:
                f.write("<?php " + version_php)
            fp_sqlite = sh([fp, "php-cli", f.name])
            os.unlink(f.name)
        print(f"SQLite der Kern-Ziele: PHP-FPM/RoadRunner {php_sqlite}, OpenGrid {og_lib}, FrankenPHP {fp_sqlite} eingebaut (gleiche Compile-Optionen; WAL, synchronous FULL, busy_timeout 5000, je Worker eine Verbindung) · ", end="")
    fp_bin = os.path.join(LARAVEL_APP, "frankenphp")
    if any("frankenphp" in t for t in a.targets.split(",")) and os.path.exists(fp_bin):
        print(f"{sh([fp_bin, 'version']).split(' Caddy')[0]} (Worker-Modus, eigenes PHP) · ", end="")
    # surrealkv commits after a grouped WAL fsync unless SURREAL_DATASTORE_SYNC says otherwise
    print(f"surrealkv sync={os.environ.get('SURREAL_DATASTORE_SYNC', 'every (Standard)')} · ", end="")
    gov = sh(["cat", "/sys/devices/system/cpu/cpu8/cpufreq/scaling_governor"]) or "?"
    cap = sh(["cat", "/sys/devices/system/cpu/cpu8/cpufreq/scaling_max_freq"])
    gov += f", max {int(cap) / 1e6:.2f} GHz" if cap.isdigit() else ""
    print(f"Governor {gov} · ", end="")
    print(f"BM2 · Commit {sh(['git', '-C', REPO, 'rev-parse', '--short', 'HEAD'])} · {datetime.datetime.now():%Y-%m-%d %H:%M} · PHP {sh(['php', '-r', 'echo PHP_VERSION;'])}"
          f" · {laravel} (Datenbank: {'Plattformschicht SurrealDB' if platform_db else ('PostgreSQL für *-pgsql, sonst SQLite' if pg_version else 'SQLite, Vorstufe')}) · SurrealDB {sh(['surreal', 'version']).split()[0]}"
          f" · {sh(['oha', '--version'])} · {a.workers} Worker je Seite · Kerne: App {a.app_cores}, DB-Server {a.db_cores}, Last {a.load_cores}"
          f" · {a.notes} Notizen vorab · je Messung {a.warmup} s Aufwärmen + {a.duration} s\n")
    results = {}
    conc = [int(c) for c in a.conc.split(",")]
    for name in a.targets.split(","):
        with tempfile.TemporaryDirectory(prefix=f"bm2-{name}-") as work:
            t = Target(name, a, work)
            try:
                t.launch()
                t.seed()
                for e in a.endpoints.split(","):
                    url, method, body, token = t.endpoint(e)
                    for c in conc:
                        oha(url, c, a.warmup, a.load_cores, method, body, token)
                        r = oha(url, c, a.duration, a.load_cores, method, body, token)
                        codes = r.get("statusCodeDistribution", {})
                        bad = sum(n for code, n in codes.items() if not code.startswith("2"))
                        lat = r["latencyPercentiles"]
                        rps = r["summary"]["requestsPerSec"]
                        # Little's law: requests in flight = throughput × mean latency; far below the
                        # connection count means connections hung and their requests are not in the
                        # percentiles (oha only counts finished requests)
                        inflight = rps * (r["summary"]["average"] or 0)
                        if c > 1 and inflight < 0.8 * c:
                            print(f"WARNING {name} {e} c={c}: only {inflight:.1f} requests in flight on average", file=sys.stderr)
                        results[(e, name, c)] = (rps, lat["p50"] * 1000, lat["p99"] * 1000, bad, sum(codes.values()), inflight)
                        print(f"  {name:<18} {e:<7} c={c:<3} {results[(e, name, c)][0]:>9.0f} req/s  p50 {results[(e, name, c)][1]:7.2f} ms  p99 {results[(e, name, c)][2]:7.2f} ms  non-2xx {bad}", file=sys.stderr)
            finally:
                t.close()
    for e in a.endpoints.split(","):
        print(f"#### {e}\n")
        print("| Ziel | Verbindungen | Anfragen/s | p50 ms | p99 ms | Fehler (nicht 2xx) | in Arbeit (Little) |")
        print("|---|---:|---:|---:|---:|---:|---:|")
        for name in a.targets.split(","):
            for c in conc:
                rps, p50, p99, bad, total, inflight = results[(e, name, c)]
                print(f"| {name} | {c} | {rps:.0f} | {p50:.2f} | {p99:.2f} | {bad} von {total} | {inflight:.1f} |")
        print()
    if a.json:
        json.dump({f"{e}|{n}|{c}": v for (e, n, c), v in results.items()}, open(a.json, "w"), indent=1)


if __name__ == "__main__":
    main()
