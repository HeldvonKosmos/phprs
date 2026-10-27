#!/usr/bin/env python3
"""Database operations per BM2 endpoint, from the databases' own logs
(BENCHMARKS §0.4): the targets of bm2.py are started and seeded the same
way, then each endpoint is requested exactly once.

- OpenGrid: the test app runs with log level `debug`; every SurrealQL query
  goes through `SurrealDb::query`, which logs it as `surreal.query`.
- Laravel: PostgreSQL logs every statement (`log_statement = all`) only
  while the endpoints are requested; a marker query separates them. The
  setting is reset afterwards.

Usage: bench/framework/querylog.py [--targets opengrid-embedded,opengrid-embedded-touch0,laravel-octane-pgsql]
Output: Markdown, one section per target and endpoint with the statements.
"""

import argparse, json, os, re, subprocess, sys, tempfile, time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import bm2  # noqa: E402

ENDPOINTS = ["health", "list", "show", "create", "login"]


def debug_config(app, workers, touch=None):
    original_patch(app, workers, touch)
    p = os.path.join(app, "config/development.yaml")
    s = open(p).read().replace("level: warn", "level: debug", 1)
    open(p, "w").write(s)


original_patch = bm2.patch_config
bm2.patch_config = debug_config


def psql(sql):
    return subprocess.run(["psql", "-At", "-d", "postgres", "-c", sql], env=dict(os.environ, **bm2.pg_env()), capture_output=True, text=True, check=True).stdout


def opengrid_queries(log, start):
    out = []
    with open(log, errors="replace") as f:
        f.seek(start)
        for line in f:
            if "surreal.query" not in line:
                continue
            try:
                rec = json.loads(line)
                fields = rec.get("fields", rec)
                sql = fields.get("sql")
            except ValueError:
                plain = re.sub(r"\x1b\[[0-9;]*m", "", line)
                m = re.search(r'sql="((?:[^"\\]|\\.)*)"', plain)
                sql = m.group(1) if m else plain.strip()
            out.append(" ".join(str(sql).split()))
    return out


# the job poller of the server process (D5) reads the jobs table about once a
# second, independent of requests: reported apart from them
POLLER = "FROM jobs WHERE reserved_at = NONE"


def pg_queries(log, start, db):
    stmts = []
    with open(log, errors="replace") as f:
        f.seek(start)
        text = f.read()
    # one entry per "LOG:  statement:" / "execute <name>:" line of the target's backends
    for m in re.finditer(r"LOG:\s+(?:statement|execute [^:]*):\s+(.*?)(?=\n\S|\Z)", text, re.S):
        s = " ".join(m.group(1).split())
        if "qlog-marker" in s or s.startswith("SET ") and "log_statement" in s:
            continue
        stmts.append(s)
    return stmts


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--targets", default="opengrid-embedded,opengrid-embedded-touch0,laravel-octane-pgsql")
    ap.add_argument("--notes", type=int, default=200)
    args = ap.parse_args()
    a = argparse.Namespace(workers=4, notes=args.notes, app_cores=bm2.APP_CORES, db_cores=bm2.DB_CORES, load_cores=bm2.LOAD_CORES,
                           opengrid=os.path.join(bm2.WS, "target/profiling/opengrid"), opengrid_pgo=os.path.join(bm2.WS, "target-pgo/opengrid-pgo"))
    if any(t.endswith("-pgsql") for t in args.targets.split(",")):
        subprocess.run([os.path.join(bm2.HERE, "pg.sh"), "start"], check=True, capture_output=True)
    # Fedora's initdb turns the logging collector on: the newest file in log/
    logdir = os.path.join(bm2.WS, ".pgdata", "log")
    pg_log = max((os.path.join(logdir, f) for f in os.listdir(logdir)), key=os.path.getmtime) if os.path.isdir(logdir) else os.path.join(bm2.WS, ".pgdata", "server.log")
    print("### Datenbankoperationen je Endpunkt (querylog.py, eine Anfrage je Endpunkt nach dem Befüllen)\n")
    for name in args.targets.split(","):
        work = tempfile.mkdtemp(prefix=f"qlog-{name}-")
        t = bm2.Target(name, a, work)
        try:
            t.launch()
            t.seed()
            # one request per endpoint, in a steady state: every endpoint is
            # requested once before the logged round
            for e in ENDPOINTS:
                url, method, body, token = t.endpoint(e)
                bm2.http(method, url, body, token)
            print(f"#### {name}\n")
            if name.startswith("opengrid"):
                log = os.path.join(work, "server.log")
                for e in ENDPOINTS:
                    time.sleep(0.3)
                    start = os.path.getsize(log)
                    url, method, body, token = t.endpoint(e)
                    st, _ = bm2.http(method, url, body, token)
                    time.sleep(0.5)
                    all_qs = opengrid_queries(log, start)
                    qs = [q for q in all_qs if POLLER not in q]
                    polled = len(all_qs) - len(qs)
                    note = f"; dazu {polled}× Job-Abfrage des Hintergrunds im selben Zeitfenster" if polled else ""
                    print(f"- **{e}** (Status {st}, {len(qs)} Abfragen{note})")
                    for q in qs:
                        print(f"  - `{q}`")
            else:
                psql("ALTER SYSTEM SET log_statement = 'all'")
                psql("SELECT pg_reload_conf()")
                time.sleep(0.5)
                try:
                    for e in ENDPOINTS:
                        start = os.path.getsize(pg_log)
                        url, method, body, token = t.endpoint(e)
                        st, _ = bm2.http(method, url, body, token)
                        time.sleep(0.5)
                        qs = pg_queries(pg_log, start, t.pg_db)
                        print(f"- **{e}** (Status {st}, {len(qs)} Anweisungen)")
                        for q in qs:
                            print(f"  - `{q}`")
                finally:
                    psql("ALTER SYSTEM RESET log_statement")
                    psql("SELECT pg_reload_conf()")
            print()
        finally:
            t.close()


if __name__ == "__main__":
    main()
