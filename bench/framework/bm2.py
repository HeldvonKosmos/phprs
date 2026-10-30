#!/usr/bin/env python3
"""BM2: framework benchmark over HTTP, OpenGrid against the Laravel comparison
application (LV1), method D11 (BENCHMARKS.md section 4).

Targets: the framework core only (D23, D25): opengrid-embedded-core,
opengrid-embedded-pgo-core (the PGO binary, --opengrid-pgo, PGO1; trained on
api.sh without the benchmark endpoints, D14), laravel-frankenphp-core
(Octane on FrankenPHP; RoadRunner and PHP-FPM are no longer measured, Jonas
28.10.), and the comparison apps of BENCH3 (bench/framework/targets.py,
started through bench/frameworks/serve.sh). All apps run the
same raw SQL on a fresh SQLite database per target (bench/framework/sqlite/:
schema, seed; WAL, synchronous FULL, busy_timeout 5000; the SQLite 3.53.4
of Bun for all targets, sqlite/build_sqlite.sh; FrankenPHP built against it), endpoints /api/bench/sql/…, the same token
check in both apps (its use written at most once a second, as Sanctum does
in effect). SurrealDB is not measured (D25): the OpenGrid targets still open
their embedded SurrealDB at start (job poller), with no queries per request.

Logic endpoints (THESIS1, core targets only): <workload>:<n> with workload
hydrate, collect, validate or json and n = 0…1000 (e.g. hydrate:100), at
/api/bench/logic/…; run with --notes 1000 so n = 1000 comes from one user's
notes. validate posts n items. Before measuring, every logic endpoint's
answer is fetched once; its size and a hash of the canonical JSON go into
the result, and a difference between targets is reported.

Endpoints: GET /health, GET /api/notes?filter[done]=false&sort=priority:desc,
GET /api/notes/{id}, POST /api/notes (all with a bearer token), POST /api/login
(bcrypt cost 10 on both sides). Load generator oha: per endpoint and
concurrency a warm-up run, then the measured run; requests/s, p50, p99.
Server, database server and load generator are pinned to separate cores; the
same number of workers on both sides.

usage: bm2.py [--targets …] [--workers 4] [--conc 1,16,64] [--duration 10] [--warmup 3] [--notes 200]
"""
import argparse, datetime, hashlib, json, os, re, shutil, signal, socket, subprocess, sys, tempfile, time, urllib.request

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import targets as fw_targets  # noqa: E402

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.abspath(os.path.join(HERE, "../.."))
WS = os.path.abspath(os.path.join(REPO, ".."))
TEST_APP = os.path.join(REPO, "examples/test-app")
SERVE = os.path.join(HERE, "laravel/serve.sh")
SERVE_FW = os.path.join(HERE, "../frameworks/serve.sh")
LARAVEL_APP = os.environ.get("LARAVEL_APP", os.path.join(WS, "bench/laravel-app"))
ALL_TARGETS = ["opengrid-embedded-core", "opengrid-embedded-pgo-core", "laravel-frankenphp-core"]
ENDPOINTS = ["health", "list", "show", "create", "login"]
# Cores (D11, Jonas 2026-10-27): the application on four physical cores, one
# SMT thread each (8, 10, 12, 14; their siblings 9, 11, 13, 15 stay idle), the
# database servers and the load generator as before
APP_CORES, DB_CORES, LOAD_CORES = "8,10,12,14", "4-7", "0-3"


def cpus(spec):
    """'8,10,12,14' or '4-7' -> [8, 10, 12, 14] / [4, 5, 6, 7]."""
    out = []
    for part in spec.split(","):
        lo, _, hi = part.partition("-")
        out += list(range(int(lo), int(hi or lo) + 1))
    return out


def topology(spec):
    """How many physical cores a CPU list covers, with the SMT siblings
    (from /sys); e.g. '4 physische Kerne, je 1 Thread'."""
    cores = {}
    for c in cpus(spec):
        try:
            sib = open(f"/sys/devices/system/cpu/cpu{c}/topology/thread_siblings_list").read().strip()
        except OSError:
            return "Topologie unbekannt"
        cores.setdefault(sib, []).append(c)
    per = sorted({len(v) for v in cores.values()})
    return f"{len(cores)} physische Kerne, je {'/'.join(map(str, per))} Thread{'s' if per != [1] else ''}"


def free_port():
    s = socket.socket()
    s.bind(("127.0.0.1", 0))
    p = s.getsockname()[1]
    s.close()
    return p


def http(method, url, body=None, token=None):
    st, raw = http_raw(method, url, body, token)
    if 200 <= st < 300:
        return st, (json.loads(raw) if raw else None)
    return st, raw.decode(errors="replace")


def http_raw(method, url, body=None, token=None):
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(url, data=data, method=method)
    if data is not None:
        req.add_header("Content-Type", "application/json")
    if token:
        req.add_header("Authorization", "Bearer " + token)
    try:
        with urllib.request.urlopen(req, timeout=30) as r:
            return r.status, r.read()
    except urllib.error.HTTPError as e:
        return e.code, e.read()


def wait_http(url, proc, what, timeout=60, log=None):
    end = time.time() + timeout
    while time.time() < end:
        if proc.poll() is not None:
            # BENCH1: the end of the server's log says why (it used to be lost
            # with the temporary directory)
            tail = open(log, errors="replace").read()[-3000:] if log and os.path.exists(log) else ""
            sys.exit(f"{what} exited early (code {proc.returncode})" + (f"\n--- {log} (end)\n{tail}" if tail else ""))
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


def patch_config(app, workers):
    p = os.path.join(app, "config/development.yaml")
    s = open(p).read()
    s = s.replace("level: info", "level: warn")
    # repeatable (startup.py patches the same copy for 1, 4, 16 workers)
    if "    debug: false\n    watch: false" not in s:
        s = s.replace("workers: 2", "workers: 2\n    debug: false\n    watch: false", 1)
    s, n = re.subn(r"workers: \d+", f"workers: {workers}", s, count=1)
    assert n == 1, "workers missing in the test app's config"
    open(p, "w").write(s)


SQLITE_LIB = os.path.join(os.path.expanduser("~"), ".local/opt/sqlite-3.53.4/lib")


_REF = None


def sqlite_reference():
    """Version and compile options of the reference SQLite, read through
    PHP's PDO as the Laravel side loads it (frameworks/check_sqlite.py)."""
    global _REF
    if _REF is None:
        sys.path.insert(0, os.path.join(HERE, "../frameworks"))
        import check_sqlite
        _REF = check_sqlite.reference()
    return _REF


def canonical(raw):
    """The answer as canonical JSON: sorted keys, and integral floats as
    integers (JavaScript has no int/float distinction: 3.0 is 3 there)."""
    def norm(x):
        if isinstance(x, float) and x.is_integer():
            return int(x)
        if isinstance(x, dict):
            return {k: norm(v) for k, v in x.items()}
        if isinstance(x, list):
            return [norm(v) for v in x]
        return x
    return json.dumps(norm(json.loads(raw)), sort_keys=True, separators=(",", ":"))


class Target:
    def __init__(self, name, a, work):
        self.name, self.a, self.work = name, a, work
        self.port = free_port()
        self.base = f"http://127.0.0.1:{self.port}"
        self.procs = []
        self.env = dict(os.environ)
        # the JIT in OpenGrid's workers: --opengrid-jit or OPENGRID_JIT=on in
        # the environment (run_all.sh; Jonas 28.10.: on for the benchmark)
        jit = getattr(a, "opengrid_jit", False) or os.environ.get("OPENGRID_JIT") in ("on", "1")
        for k in ("OPENGRID_DB_URL", "SURREAL_DB_HOST", "GRD_JIT", "OPENGRID_JIT"):
            self.env.pop(k, None)
        if jit and name.startswith("opengrid"):
            self.env["OPENGRID_JIT"] = "on"
        # D23 (DBR1): the framework core with the same raw SQL on SQLite; the
        # SurrealDB targets and Laravel's standard setups are gone (D25)
        if not name.endswith("-core"):
            sys.exit(f"{name}: only -core targets are measured (D25: SQLite only, SurrealDB not measured)")
        self.core = True
        i = fw_targets.info(name)
        if i is None:
            sys.exit(f"{name}: unknown target (bench/framework/targets.py)")
        self.app, self.variant = i[3], i[4]

    def launch(self):
        w = self.a.workers
        # a fresh SQLite database of the same schema and data for both apps
        db = os.path.join(self.work, "bench.sqlite")
        subprocess.run(["python3", os.path.join(HERE, "sqlite", "seed.py"), db, str(self.a.notes)], check=True, capture_output=True)
        self.env.update(OPENGRID_DB_BENCH_SQLITE_URL=f"sqlite://{db}", BENCH_SQLITE_DATABASE=db)
        # the benchmark's SQLite (Bun's 3.53.4 with its compile options,
        # sqlite/build_sqlite.sh) for every side that loads a shared libsqlite3
        self.env["LD_LIBRARY_PATH"] = SQLITE_LIB + (":" + self.env["LD_LIBRARY_PATH"] if self.env.get("LD_LIBRARY_PATH") else "")
        if self.name.startswith("opengrid"):
            app = os.path.join(self.work, "app")
            shutil.copytree(TEST_APP, app, ignore=shutil.ignore_patterns(".opengrid", "tests"))
            patch_config(app, w)
            # the framework's own embedded SurrealDB (job poller; no queries
            # per request of the core endpoints), in the target's directory
            self.env["OPENGRID_DB_URL"] = f"surrealkv://{self.work}/db"
            binary = self.a.opengrid_pgo if "-pgo" in self.name else self.a.opengrid
            r = subprocess.run([binary, "task", "migrate_push"], cwd=app, env=self.env, capture_output=True, text=True)
            if r.returncode != 0:
                sys.exit(f"migrate_push failed: {r.stdout[-800:]}{r.stderr[-800:]}")
            p = start([binary, "start", "--no-banner", "--binding", "127.0.0.1", "--port", str(self.port)], self.a.app_cores, self.env, f"{self.work}/server.log", cwd=app)
        else:
            # Laravel's default connection (migrations, cache, sessions) on its own SQLite file
            dflt = os.path.join(self.work, "db.sqlite")
            open(dflt, "w").close()
            self.env.update(DB_CONNECTION="sqlite", DB_DATABASE=dflt)
        if self.name.startswith("laravel"):
            subprocess.run(["php", "artisan", "config:clear"], cwd=LARAVEL_APP, env=self.env, capture_output=True)
            r = subprocess.run(["php", "artisan", "migrate", "--force"], cwd=LARAVEL_APP, env=self.env, capture_output=True, text=True)
            if r.returncode != 0:
                sys.exit(f"artisan migrate failed: {r.stdout[-800:]}{r.stderr[-800:]}")
            mode = self.name.split("-")[1]
            # RoadRunner's RPC / FrankenPHP's admin port: a port found free, not port + 1
            env = dict(self.env, SIDE_PORT=str(free_port()))
            p = start([SERVE, mode, str(self.port), os.path.join(self.work, "run"), str(w)], self.a.app_cores, env, f"{self.work}/server.log")
        if self.app:
            # BENCH3: the comparison apps, started as bench/frameworks/serve.sh does
            p = start(["bash", SERVE_FW, self.app, str(self.port), os.path.join(self.work, "run"), str(w)] + ([self.variant] if self.variant else []), self.a.app_cores, self.env, f"{self.work}/server.log")
        self.procs.append(p)
        wait_http(self.base + "/health", p, self.name, timeout=300, log=f"{self.work}/server.log")
        self.check_sqlite()

    def check_sqlite(self):
        """BENCH3: every side on the reference SQLite (version and compile
        options); the side's runtime goes into the result's header."""
        st, info = http("GET", self.base + "/api/bench/sql/sqlite")
        assert st == 200, (self.name, st, info)
        ref = sqlite_reference()
        # COMPILER=… names the compiler version only (FrankenPHP builds its own
        # SQLite with a newer gcc); every other option must match
        opts = lambda o: sorted(x for x in o if not x.startswith("COMPILER="))
        if info["version"] != ref["version"] or opts(info["compile_options"]) != opts(ref["compile_options"]):
            diff = sorted(set(opts(info["compile_options"])) ^ set(opts(ref["compile_options"])))
            sys.exit(f"{self.name}: SQLite {info['version']} with other compile options than the reference {ref['version']} ({diff[:6]})")
        self.info = info
        self.info.setdefault("runtime", {})["sqlite compiler"] = next((x[9:] for x in info["compile_options"] if x.startswith("COMPILER=")), "?")

    def seed(self):
        # the data comes from seed.py; the token from the core's login
        st, body = http("POST", self.base + "/api/bench/sql/login", {"email": "bench@example.com", "password": "secret-123"})
        assert st == 200, (st, body)
        self.token = body["token"]
        self.note_id = str(self.a.notes // 2 + 1)

    def endpoint(self, e):
        b, t = self.base, self.token
        if ":" in e:
            # THESIS1: a logic workload with n
            assert self.core, f"{e}: logic endpoints only on -core targets"
            w, n = e.split(":")
            n = int(n)
            if w == "validate":
                items = [{"title": f"Item {i}", "priority": i % 5 + 1, "tags": ["a", "b"]} for i in range(n)]
                return (b + "/api/bench/logic/validate", "POST", {"items": items}, t)
            assert w in ("hydrate", "collect", "json"), w
            return (b + f"/api/bench/logic/{w}?n={n}", "GET", None, t)
        c = b + "/api/bench/sql"
        return {
            "health": (c + "/health", "GET", None, None),
            "list": (c + "/notes?done=false&per_page=15", "GET", None, t),
            "show": (c + f"/notes/{self.note_id}", "GET", None, t),
            "create": (c + "/notes", "POST", {"title": "Bench", "body": "created by bm2", "priority": 2}, t),
            "login": (c + "/login", "POST", {"email": "bench@example.com", "password": "secret-123"}, None),
        }[e]

    def close(self):
        for p in reversed(self.procs):
            stop(p)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--targets", default=",".join(ALL_TARGETS))
    ap.add_argument("--endpoints", default=",".join(ENDPOINTS))
    ap.add_argument("--workers", type=int, default=4)
    ap.add_argument("--conc", default="1,16,64")
    ap.add_argument("--duration", type=int, default=10)
    ap.add_argument("--warmup", type=int, default=3)
    ap.add_argument("--notes", type=int, default=200)
    ap.add_argument("--app-cores", default=APP_CORES)
    ap.add_argument("--db-cores", default=DB_CORES)
    ap.add_argument("--load-cores", default=LOAD_CORES)
    ap.add_argument("--opengrid", default=os.path.join(WS, "target/perf/opengrid"))
    ap.add_argument("--opengrid-pgo", default=os.path.join(WS, "target-pgo/opengrid-pgo"))
    ap.add_argument("--json", help="write the raw results here")
    ap.add_argument("--opengrid-jit", action="store_true", help="OPENGRID_JIT=on for the OpenGrid targets (PERF4 d; default off, D9)")
    ap.add_argument("--label", default="", help="heading of the result")
    a = ap.parse_args()
    a.opengrid_jit = a.opengrid_jit or os.environ.get("OPENGRID_JIT") in ("on", "1")
    a.opengrid = os.path.abspath(a.opengrid)
    a.opengrid_pgo = os.path.abspath(a.opengrid_pgo)
    sh = lambda c, cwd=None: subprocess.run(c, capture_output=True, text=True, cwd=cwd).stdout.strip()
    laravel = sh(["php", "artisan", "--version"], LARAVEL_APP) if os.path.isdir(LARAVEL_APP) else "–"
    if a.label:
        print(f"### {a.label}\n")
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
    print(f"SQLite of every side (D23, BENCH3): reference {php_sqlite} (Bun's SQLite with Bun's compile options, {SQLITE_LIB}), OpenGrid loads {og_lib}, FrankenPHP (self-built) {fp_sqlite}; every side checked against the reference before measuring (runtimes per target below); WAL, synchronous FULL, busy_timeout 5000 · ", end="")
    fp_bin = os.path.join(LARAVEL_APP, "frankenphp")
    if any("frankenphp" in t for t in a.targets.split(",")) and os.path.exists(fp_bin):
        print(f"{sh([fp_bin, 'version']).split(' Caddy')[0]} (Worker-Modus, eigenes PHP) · ", end="")
    # D25: SurrealDB is not measured; OpenGrid still opens its embedded one at start (job poller)
    print(f"SurrealDB not measured (D25; OpenGrid opens its embedded SurrealDB {sh(['surreal', 'version']).split()[0]} at start for the job poller, no queries per request) · ", end="")
    gov = sh(["cat", "/sys/devices/system/cpu/cpu8/cpufreq/scaling_governor"]) or "?"
    cap = sh(["cat", "/sys/devices/system/cpu/cpu8/cpufreq/scaling_max_freq"])
    gov += f", max {int(cap) / 1e6:.2f} GHz" if cap.isdigit() else ""
    print(f"governor {gov} · ", end="")
    print(f"JIT of the OpenGrid workers {'on (OPENGRID_JIT=on; 2026-10-28)' if a.opengrid_jit else 'off (D9)'} · ", end="")
    # which PHP runs which target (FrankenPHP brings its own, newer PHP)
    fp_php = re.search(r"\bPHP (\d\S*)", sh([fp_bin, "version"])).group(1) if os.path.exists(fp_bin) else "–"
    print(f"BM2 · Commit {sh(['git', '-C', REPO, 'rev-parse', '--short', 'HEAD'])} · {datetime.datetime.now():%Y-%m-%d %H:%M} · PHP {fp_php} in FrankenPHP (self-built, ZTS), PHP {sh(['php', '-r', 'echo PHP_VERSION;'])} for artisan"
          f" · {laravel} (Datenbank: SQLite)"
          f" · {sh(['oha', '--version'])} · {a.workers} workers per side · cores: app {a.app_cores} ({topology(a.app_cores)}), database server {a.db_cores} ({topology(a.db_cores)}), load {a.load_cores} ({topology(a.load_cores)})"
          f" · {a.notes} notes seeded · per measurement {a.warmup} s warm-up + {a.duration} s\n")
    results = {}
    runtimes = {}
    answers = {}
    conc = [int(c) for c in a.conc.split(",")]
    for name in a.targets.split(","):
        with tempfile.TemporaryDirectory(prefix=f"bm2-{name}-") as work:
            t = Target(name, a, work)
            try:
                t.launch()
                t.seed()
                runtimes[name] = {"sqlite": t.info["version"], **t.info.get("runtime", {})}
                for e in a.endpoints.split(","):
                    url, method, body, token = t.endpoint(e)
                    if ":" in e:
                        st, raw = http_raw(method, url, body, token)
                        canon = canonical(raw)
                        answers[(e, name)] = (st, len(raw), hashlib.sha256(canon.encode()).hexdigest()[:12])
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
                        # no finished request at all (oha reports no percentiles): latency
                        # counts as the whole measurement, and it is warned about
                        if lat.get("p50") is None:
                            print(f"WARNING {name} {e} c={c}: no request finished within {a.duration} s", file=sys.stderr)
                        p50, p99 = ((lat.get(q) or float(a.duration)) * 1000 for q in ("p50", "p99"))
                        results[(e, name, c)] = (rps, p50, p99, bad, sum(codes.values()), inflight)
                        print(f"  {name:<18} {e:<7} c={c:<3} {results[(e, name, c)][0]:>9.0f} req/s  p50 {results[(e, name, c)][1]:7.2f} ms  p99 {results[(e, name, c)][2]:7.2f} ms  non-2xx {bad}", file=sys.stderr)
            finally:
                t.close()
    for e in a.endpoints.split(","):
        print(f"#### {e}\n")
        print("| Target | Connections | Requests/s | p50 ms | p99 ms | Errors (non-2xx) | In flight (Little) |")
        print("|---|---:|---:|---:|---:|---:|---:|")
        for name in a.targets.split(","):
            for c in conc:
                rps, p50, p99, bad, total, inflight = results[(e, name, c)]
                print(f"| {name} | {c} | {rps:.0f} | {p50:.2f} | {p99:.2f} | {bad} von {total} | {inflight:.1f} |")
        print()
    print("#### Runtimes per target (BENCH3; SQLite of every side checked against the reference)\n")
    print("| Target | SQLite | Runtime |")
    print("|---|---|---|")
    for name, r in runtimes.items():
        print(f"| {name} | {r['sqlite']} | {', '.join(f'{k} {v}' for k, v in r.items() if k != 'sqlite')} |")
    print()
    if answers:
        print("#### Answers of the logic endpoints (fetched once before measuring)\n")
        print("| Endpoint | Target | Status | Bytes | Hash (canonical JSON) |")
        print("|---|---|---:|---:|---|")
        for (e, name), (st, size, h) in sorted(answers.items()):
            print(f"| {e} | {name} | {st} | {size} | {h} |")
        for e in sorted({e for e, _ in answers}):
            if len({v[2] for (e2, _), v in answers.items() if e2 == e}) > 1:
                print(f"\n**Difference:** `{e}` does not answer the same on every target.")
                print(f"WARNING {e}: answers differ between targets", file=sys.stderr)
        print()
    if a.json:
        out = {f"{e}|{n}|{c}": v for (e, n, c), v in results.items()}
        out.update({f"answer|{e}|{n}": v for (e, n), v in answers.items()})
        out.update({f"runtime|{n}": v for n, v in runtimes.items()})
        json.dump(out, open(a.json, "w"), indent=1)


if __name__ == "__main__":
    main()
