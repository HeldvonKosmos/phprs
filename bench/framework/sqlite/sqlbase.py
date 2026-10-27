#!/usr/bin/env python3
"""Database baseline of the framework core (THESIS1; BENCHMARKS §0.4 method
for SQLite): how long the SQLite statements of one request of each core
endpoint take by themselves, without the framework, on a database seeded
as bm2.py seeds the -core targets (seed.py, 1 user, --notes notes, one
token). Both drivers against the same SQLite library (build_sqlite.sh,
3.45.2, through LD_LIBRARY_PATH), one connection, on one application core:
- php: sqlbase.php, PDO sqlite with Laravel's options (as in the Laravel app),
- rust: the `sqlbase` example, rusqlite with prepare per call (as OpenGrid).
The token's use is written at most once a second, as by BenchToken.

  sqlbase.py [--endpoints …] [--notes 1000] [--warmup 2] [--duration 5] [--json FILE]
Needs `cargo build --profile profiling -p opengrid-framework --example sqlbase`.
"""

import argparse, hashlib, json, os, sqlite3, subprocess, sys, tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.dirname(HERE))
import bm2  # noqa: E402

RUST = os.path.join(bm2.WS, "target/profiling/examples/sqlbase")
PHP = os.path.join(HERE, "sqlbase.php")
# create last: it adds notes
ENDPOINTS = "health,list,show,hydrate:0,hydrate:1,hydrate:10,hydrate:100,hydrate:1000,json:0,create"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--endpoints", default=ENDPOINTS)
    ap.add_argument("--notes", type=int, default=1000)
    ap.add_argument("--warmup", type=float, default=2)
    ap.add_argument("--duration", type=float, default=5)
    ap.add_argument("--core", default="8", help="the application core the drivers run on")
    ap.add_argument("--json", help="write the raw results here")
    a = ap.parse_args()
    if not os.path.exists(RUST):
        sys.exit(f"missing {RUST}: cargo build --profile profiling -p opengrid-framework --example sqlbase")
    if not os.path.exists(os.path.join(bm2.SQLITE_LIB, "libsqlite3.so.0")):
        subprocess.run([os.path.join(HERE, "build_sqlite.sh")], check=True)
    env = dict(os.environ, LD_LIBRARY_PATH=bm2.SQLITE_LIB)
    res = {}
    with tempfile.TemporaryDirectory(prefix="sqlbase-") as work:
        db = os.path.join(work, "bench.sqlite")
        subprocess.run(["python3", os.path.join(HERE, "seed.py"), db, str(a.notes)], check=True, capture_output=True)
        token_hash = hashlib.sha256(b"sqlbase-token").hexdigest()
        c = sqlite3.connect(db)
        c.execute("INSERT INTO tokens (user_id, token_hash, last_used_at, created_at) VALUES (1, ?, 0, '2026-10-27 00:00:00')", (token_hash,))
        c.commit()
        c.close()
        show = f"show:{a.notes // 2 + 1}"
        for e in a.endpoints.split(","):
            arg = show if e == "show" else e
            for side, cmd in (("php", ["php", PHP]), ("rust", [RUST])):
                out = subprocess.run(["taskset", "-c", a.core] + cmd + [db, token_hash, arg, str(a.warmup), str(a.duration)], capture_output=True, text=True, env=env)
                if out.returncode != 0:
                    sys.exit(f"sqlbase {side} {e}: {out.stderr[-500:]}")
                res[f"{e}|{side}"] = json.loads(out.stdout)
                print(f"  {e:<13} {side:<4} p50 {res[f'{e}|{side}']['p50_ms']:.3f} ms", file=sys.stderr)
    php_sqlite = subprocess.run(["php", "-r", "echo (new PDO('sqlite::memory:'))->query('select sqlite_version()')->fetchColumn();"], capture_output=True, text=True, env=env).stdout.strip()
    print(f"SQLite {php_sqlite} (PDO und rusqlite, LD_LIBRARY_PATH), {a.notes} Notizen, 1 Verbindung, Kern {a.core}, {a.warmup} s Aufwärmen + {a.duration} s\n")
    print("| Endpunkt | PDO (wie Laravel) p50 ms | p99 ms | rusqlite (wie OpenGrid) p50 ms | p99 ms |")
    print("|---|---:|---:|---:|---:|")
    for e in a.endpoints.split(","):
        p, r = res[f"{e}|php"], res[f"{e}|rust"]
        print(f"| {e} | {p['p50_ms']:.3f} | {p['p99_ms']:.3f} | {r['p50_ms']:.3f} | {r['p99_ms']:.3f} |")
    if a.json:
        json.dump(res, open(a.json, "w"), indent=1)


if __name__ == "__main__":
    main()
