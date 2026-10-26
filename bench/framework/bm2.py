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
  <opengrid target>-touch0
                     any OpenGrid target with auth.token_touch_seconds = 0 (the
                     token's last use written on every request, as Sanctum),
                     e.g. opengrid-embedded-touch0, opengrid-server-touch0
The Laravel app uses its database from the environment: SQLite until the
platform's SurrealDB layer is installed (setup.sh), then the same SurrealDB
server as OpenGrid.

Endpoints: GET /health, GET /api/notes?filter[done]=false&sort=priority:desc,
GET /api/notes/{id}, POST /api/notes (all with a bearer token), POST /api/login
(bcrypt cost 10 on both sides). Load generator oha: per endpoint and
concurrency a warm-up run, then the measured run; requests/s, p50, p99.
Server, database server and load generator are pinned to separate cores; the
same number of workers on both sides.

usage: bm2.py [--targets …] [--workers 4] [--conc 1,16,64] [--duration 10] [--warmup 3] [--notes 200]
"""
import argparse, datetime, json, os, secrets, shutil, signal, socket, subprocess, sys, tempfile, time, urllib.request

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


PG_LOCAL = os.path.join(WS, ".pgdata")


def pg_env():
    # the in-container server of pg.sh, another one only through an explicit
    # PGHOST; credentials only through the environment
    user = os.environ.get("PGUSER", os.environ.get("USER", "dev"))
    return {"PGHOST": os.environ.get("PGHOST", PG_LOCAL), "PGPORT": os.environ.get("PGPORT", "5432"), "PGUSER": user, "PGPASSWORD": os.environ.get("PGPASSWORD", "")}


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
        if self.name.startswith("opengrid"):
            app = os.path.join(self.work, "app")
            shutil.copytree(TEST_APP, app, ignore=shutil.ignore_patterns(".opengrid", "tests"))
            patch_config(app, w, 0 if self.name.endswith("-touch0") else None)
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
    ap.add_argument("--label", default="", help="heading of the result, e.g. 'Vergleich der Standard-Setups'")
    a = ap.parse_args()
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
        where = "im Workbench-Container (pg.sh), Kerne der DB-Server" if pg_env()["PGHOST"] == PG_LOCAL else f"Server {pg_env()['PGHOST']}, nicht gepinnt"
        print(f"PostgreSQL {pg_version} ({where}) · ", end="")
    gov = sh(["cat", "/sys/devices/system/cpu/cpu8/cpufreq/scaling_governor"]) or "?"
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
