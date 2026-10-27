#!/usr/bin/env python3
"""Database baseline for BM2 (BENCHMARKS §0.4): how fast the databases are
by themselves for the statements each BM2 endpoint runs per request
(bench/framework/querylog.py), without the framework, with the same data,
connection and cores.

Each target is set up and seeded exactly as by bm2.py; then only the
application server is stopped and the database stays:
- opengrid-embedded: the `dbbase` example opens the same surrealkv:// files
  in its own process (on the application cores, like the app),
- opengrid-server: `dbbase` against the same SurrealDB server (ws://),
- laravel-octane-pgsql / laravel-fpm-pgsql: dbbase.php with PDO pgsql over
  the Unix socket, one connection per client (Octane) or a new connection
  per request (FPM).
Variants `+touch`: the token use written on every request (OpenGrid touch0;
Sanctum's update, which in the benchmark happens at most once a second).

Usage: bench/framework/dbbase.py [--targets …] [--endpoints …] [--conc 1,16,64] [--warmup 3] [--duration 10] [--json FILE]
Needs `cargo build --profile profiling -p opengrid-framework --example dbbase`.
"""

import argparse, json, os, subprocess, sys, tempfile

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import bm2  # noqa: E402

TARGETS = ["opengrid-embedded", "opengrid-server", "laravel-octane-pgsql", "laravel-fpm-pgsql"]
ENDPOINTS = ["health", "list", "show", "create", "login"]
DBBASE = os.path.join(bm2.WS, "target/profiling/examples/dbbase")
PHP = os.path.join(bm2.HERE, "dbbase.php")


def variants(target, endpoint):
    if endpoint == "health":
        # OpenGrid's health endpoint touches no database; FPM connects anyway
        return ["health"] if target == "laravel-fpm-pgsql" else []
    if endpoint == "login":
        return ["login"]
    return [endpoint, endpoint + "+touch"]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--targets", default=",".join(TARGETS))
    ap.add_argument("--endpoints", default=",".join(ENDPOINTS))
    ap.add_argument("--conc", default="1,16,64")
    ap.add_argument("--warmup", type=float, default=3)
    ap.add_argument("--duration", type=float, default=10)
    ap.add_argument("--notes", type=int, default=200)
    ap.add_argument("--json", help="write the raw results here")
    args = ap.parse_args()
    a = argparse.Namespace(workers=4, notes=args.notes, app_cores="8-11", db_cores="4-7", load_cores="0-3",
                           opengrid=os.path.join(bm2.WS, "target/profiling/opengrid"), opengrid_pgo=os.path.join(bm2.WS, "target-pgo/opengrid-pgo"))
    if not os.path.exists(DBBASE):
        sys.exit(f"missing {DBBASE}: cargo build --profile profiling -p opengrid-framework --example dbbase")
    if any(t.endswith("-pgsql") for t in args.targets.split(",")):
        subprocess.run([os.path.join(bm2.HERE, "pg.sh"), "start"], check=True, capture_output=True)
    results = {}
    print("| Ziel | Abfragen | Verbindungen | Durchläufe/s | p50 ms | p99 ms |\n|---|---|---:|---:|---:|---:|")
    for name in args.targets.split(","):
        work = tempfile.mkdtemp(prefix=f"dbbase-{name}-")
        t = bm2.Target(name, a, work)
        try:
            t.launch()
            t.seed()
            # only the application goes; the database (server) stays
            bm2.stop(t.procs.pop())
            env = dict(t.env)
            if name == "opengrid-embedded":
                cmd = lambda v, c: ["taskset", "-c", a.app_cores, DBBASE, f"surrealkv://{work}/db", v, str(c), str(args.warmup), str(args.duration)]
            elif name == "opengrid-server":
                url = f"ws://{env['SURREAL_DB_HOST']}:{env['SURREAL_DB_PORT']}"
                cmd = lambda v, c, url=url: ["taskset", "-c", a.app_cores, DBBASE, url, v, str(c), str(args.warmup), str(args.duration)]
            else:
                env.update(bm2.pg_env(), PGDATABASE=t.pg_db)
                mode = "persistent" if "octane" in name else "connect"
                cmd = lambda v, c, mode=mode: ["taskset", "-c", a.app_cores, "php", PHP, v, str(c), str(args.warmup), str(args.duration), mode]
            for e in args.endpoints.split(","):
                for v in variants(name, e):
                    for c in [int(x) for x in args.conc.split(",")]:
                        r = subprocess.run(cmd(v, c), env=env, capture_output=True, text=True)
                        if r.returncode != 0:
                            sys.exit(f"{name} {v} c={c} failed: {r.stdout[-600:]}{r.stderr[-600:]}")
                        res = json.loads(r.stdout.strip().splitlines()[-1])
                        results[f"{v}|{name}|{c}"] = res
                        print(f"| {name} | {v} | {c} | {res['ops_per_s']:.0f} | {res['p50_ms']:.2f} | {res['p99_ms']:.2f} |", flush=True)
                        print(f"  {name:22} {v:13} c={c:<3} {res['ops_per_s']:9.0f} ops/s  p50 {res['p50_ms']:7.2f} ms  p99 {res['p99_ms']:7.2f} ms", file=sys.stderr, flush=True)
        finally:
            t.close()
    if args.json:
        json.dump(results, open(args.json, "w"), indent=1)


if __name__ == "__main__":
    main()
