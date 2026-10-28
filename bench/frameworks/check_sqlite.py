#!/usr/bin/env python3
"""Checks that a comparison app runs the benchmark's SQLite (BENCH3): starts
it through serve.sh on a seeded database, reads GET /api/bench/sql/sqlite
and compares the version and `pragma compile_options` with the reference
library (bench/framework/sqlite/build_sqlite.sh, read through PHP's PDO as
the Laravel side loads it). Prints the app's runtime information.

  check_sqlite.py <app>[:<variant>] ...      (exit 1 on any mismatch)
As a module: reference() and check(app, variant) for the benchmark tools.
"""

import json, os, signal, socket, subprocess, sys, tempfile, time, urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))
BENCH = os.path.dirname(HERE)
LIB = os.path.expanduser("~/.local/opt/sqlite-3.53.4/lib")


def reference():
    code = "$p=new PDO('sqlite::memory:'); echo json_encode(['version'=>$p->query('select sqlite_version()')->fetchColumn(),'compile_options'=>$p->query('pragma compile_options')->fetchAll(PDO::FETCH_COLUMN)]);"
    return json.loads(subprocess.run(["php", "-r", code], capture_output=True, text=True, check=True, env=dict(os.environ, LD_LIBRARY_PATH=LIB)).stdout)


def free_port():
    s = socket.socket(); s.bind(("127.0.0.1", 0)); p = s.getsockname()[1]; s.close(); return p


def check(app, variant="", ref=None):
    ref = ref or reference()
    with tempfile.TemporaryDirectory(prefix=f"sqlite-{app}-") as w:
        db = os.path.join(w, "bench.sqlite")
        subprocess.run(["python3", os.path.join(BENCH, "framework", "sqlite", "seed.py"), db, "4"], check=True, capture_output=True)
        port = free_port()
        p = subprocess.Popen(["bash", os.path.join(HERE, "serve.sh"), app, str(port), os.path.join(w, "run"), "4"] + ([variant] if variant else []),
                             env=dict(os.environ, BENCH_SQLITE_DATABASE=db), stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True)
        try:
            info = None
            for _ in range(600):
                try:
                    with urllib.request.urlopen(f"http://127.0.0.1:{port}/api/bench/sql/sqlite", timeout=2) as r:
                        info = json.loads(r.read())
                        break
                except Exception:
                    if p.poll() is not None:
                        raise SystemExit(f"{app}: server exited ({p.returncode})")
                    time.sleep(0.1)
            if info is None:
                raise SystemExit(f"{app}: no answer")
        finally:
            os.killpg(p.pid, signal.SIGTERM)
            p.wait(15)
    # COMPILER=… names the compiler version only; every other option must match
    opts = lambda o: sorted(x for x in o if not x.startswith("COMPILER="))
    ok = info["version"] == ref["version"] and opts(info["compile_options"]) == opts(ref["compile_options"])
    return ok, info


def main():
    ref = reference()
    bad = 0
    for spec in sys.argv[1:]:
        app, _, variant = spec.partition(":")
        ok, info = check(app, variant, ref)
        extra = sorted(set(info["compile_options"]) ^ set(ref["compile_options"]))
        print(f"{spec:<18} SQLite {info['version']:<7} {len(info['compile_options'])} options {'= reference' if ok else 'DIFFERS ' + str(extra[:4])}  runtime {info.get('runtime')}")
        bad += not ok
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
