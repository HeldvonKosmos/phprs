#!/usr/bin/env python3
"""Cold start and bootstrap (BENCH1), per target of bm2.py (set up and
seeded as there; then only the application server is restarted, the data
stays):
- start: from starting the server command to the first 200 of the empty
  route /api/bench/sql/health (no database, no middleware), with 4 and 16
  workers; Laravel with its caches built beforehand (SERVE_CACHES=keep, as
  after a deploy) and without caches (SERVE_CACHES=none);
- first request: right after that, one request of --endpoint (token,
  database) against the median of the next --warm requests;
- hot reload: a controller file is changed (the empty route's answer), and
  the time until the server answers with the new code is taken; each stack
  as it offers it: OpenGrid's file watcher (watch: true), PHP-FPM through
  opcache revalidation (opcache defaults: validate_timestamps=1,
  revalidate_freq=2), Octane (RoadRunner, FrankenPHP) through
  `php artisan octane:reload`, since they watch no files without --watch
  and Node. The file is restored afterwards. The comparison apps (BENCH3)
  are measured in production mode here; their hot reload is measured in
  each framework's development mode by bench/frameworks/hotreload.py.

  coldstart.py [--targets …] [--runs 5] [--workers 4,16] [--endpoint show] [--warm 50] [--json FILE]
"""

import argparse, json, os, re, statistics, subprocess, sys, tempfile, time, urllib.request

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import bm2  # noqa: E402

TARGETS = ["opengrid-embedded-pgo-core", "laravel-frankenphp-core"]
OG_CONTROLLER = "modules/benchsql/app/Controllers/BenchSqlController.grd"
LV_CONTROLLER = "app/Http/Controllers/BenchSqlController.php"
OK = "return Response::json(['status' => 'ok']);"
OK_LV = "return response()->json(['status' => 'ok']);"


def get(url, token=None, timeout=1):
    req = urllib.request.Request(url)
    if token:
        req.add_header("Authorization", "Bearer " + token)
    with urllib.request.urlopen(req, timeout=timeout) as r:
        return r.status, r.read()


def until(url, proc, want=None, timeout=120):
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        if proc is not None and proc.poll() is not None:
            sys.exit(f"server exited with {proc.returncode}")
        try:
            st, body = get(url)
            if st == 200 and (want is None or want in body):
                return
        except Exception:
            pass
        time.sleep(0.002)
    sys.exit(f"no answer{' with ' + want.decode() if want else ''} from {url} within {timeout} s")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--targets", default=",".join(TARGETS))
    ap.add_argument("--runs", type=int, default=5)
    ap.add_argument("--workers", default="4,16")
    ap.add_argument("--endpoint", default="show")
    ap.add_argument("--warm", type=int, default=50)
    ap.add_argument("--notes", type=int, default=200)
    ap.add_argument("--opengrid", default=os.path.join(bm2.WS, "target/perf/opengrid"))
    ap.add_argument("--opengrid-pgo", default=os.path.join(bm2.WS, "target-pgo/opengrid-pgo"))
    ap.add_argument("--json", help="write the raw results here")
    a = ap.parse_args()
    a.app_cores, a.db_cores, a.load_cores, a.workers_list = bm2.APP_CORES, bm2.DB_CORES, bm2.LOAD_CORES, [int(w) for w in a.workers.split(",")]
    a.workers = a.workers_list[0]
    a.opengrid, a.opengrid_pgo = os.path.abspath(a.opengrid), os.path.abspath(a.opengrid_pgo)
    res = {}
    # remember how bm2 starts the application server, to start it again
    calls = []
    orig_start = bm2.start

    def recording_start(cmd, cores, env, log, cwd=None):
        calls.append((cmd, cores, env, log, cwd))
        return orig_start(cmd, cores, env, log, cwd)
    bm2.start = recording_start
    for name in a.targets.split(","):
        laravel = name.startswith("laravel")
        with tempfile.TemporaryDirectory(prefix=f"cold-{name}-") as work:
            t = bm2.Target(name, a, work)
            try:
                t.launch()
                t.seed()
                cmd, cores, env, log, cwd = calls[-1]
                empty = t.base + "/api/bench/sql/health"
                url, method, body, token = t.endpoint(a.endpoint)
                assert method == "GET", "the first-request check uses a GET endpoint"
                r = res.setdefault(name, {"start": {}, "first": {}, "warm": {}, "reload": None})
                for w in a.workers_list:
                    for caches in (["keep", "none"] if laravel else ["-"]):
                        starts, firsts, warms = [], [], []
                        for _ in range(a.runs):
                            bm2.stop(t.procs.pop())
                            if t.app:
                                # bench/frameworks/serve.sh <app> <port> <dir> <workers> [variant]
                                c, e = cmd[:5] + [str(w)] + cmd[6:], env
                            elif laravel:
                                c = cmd[:-1] + [str(w)]
                                e = dict(env, SERVE_CACHES=caches)
                            else:
                                bm2.patch_config(os.path.join(work, "app"), w)
                                c, e = cmd, env
                            t0 = time.monotonic()
                            p = orig_start(c, cores, e, log, cwd)
                            t.procs.append(p)
                            until(empty, p)
                            starts.append((time.monotonic() - t0) * 1000)
                            t1 = time.monotonic()
                            st, _ = get(url, token, timeout=30)
                            assert st == 200, st
                            firsts.append((time.monotonic() - t1) * 1000)
                            lat = []
                            for _ in range(a.warm):
                                t2 = time.monotonic()
                                get(url, token, timeout=30)
                                lat.append((time.monotonic() - t2) * 1000)
                            warms.append(statistics.median(lat))
                        key = f"{w}|{caches}"
                        r["start"][key], r["first"][key], r["warm"][key] = starts, firsts, warms
                        print(f"  {name:<26} workers {w:<2} caches {caches:<4} start {statistics.median(starts):6.0f} ms  first {statistics.median(firsts):6.2f} ms  warm {statistics.median(warms):5.2f} ms", file=sys.stderr)
                if t.app:
                    continue
                # hot reload, with the first worker count; restore the caches first (Laravel)
                w = a.workers_list[0]
                bm2.stop(t.procs.pop())
                if laravel:
                    # opcache with timestamp checks and no preload, so the changed file is seen (SERVE_PHP_DEV)
                    c, e = cmd[:-1] + [str(w)], dict(env, SERVE_CACHES="build", SERVE_PHP_DEV="1")
                    path, old = os.path.join(bm2.LARAVEL_APP, LV_CONTROLLER), OK_LV
                else:
                    app = os.path.join(work, "app")
                    bm2.patch_config(app, w)
                    conf = os.path.join(app, "config/development.yaml")
                    text = open(conf).read()
                    open(conf, "w").write(text.replace("watch: false", "watch: true"))
                    c, e = cmd, env
                    path, old = os.path.join(app, OG_CONTROLLER), OK
                p = orig_start(c, cores, e, log, cwd)
                t.procs.append(p)
                until(empty, p)
                src = open(path).read()
                assert old in src, f"{path}: health answer not found"
                times = []
                try:
                    for i in range(1, 4):
                        mark = f"ok{i}"
                        open(path, "w").write(src.replace(old, old.replace("'ok'", f"'{mark}'")))
                        t0 = time.monotonic()
                        if name.startswith(("laravel-octane", "laravel-frankenphp")):
                            server = "frankenphp" if "frankenphp" in name else "roadrunner"
                            subprocess.run(["php", "artisan", "octane:reload", f"--server={server}"], cwd=bm2.LARAVEL_APP, env=e, capture_output=True, check=True)
                        until(empty, p, want=mark.encode(), timeout=30)
                        times.append((time.monotonic() - t0) * 1000)
                finally:
                    open(path, "w").write(src)
                    if laravel and name.startswith(("laravel-octane", "laravel-frankenphp")):
                        subprocess.run(["php", "artisan", "octane:reload", f"--server={'frankenphp' if 'frankenphp' in name else 'roadrunner'}"], cwd=bm2.LARAVEL_APP, env=e, capture_output=True)
                how = "octane:reload" if name.startswith(("laravel-octane", "laravel-frankenphp")) else ("opcache revalidation" if laravel else "file watcher")
                r["reload"] = {"ms": times, "how": how}
                print(f"  {name:<26} hot reload ({how}) {statistics.median(times):.0f} ms", file=sys.stderr)
            finally:
                t.close()
    bm2.start = orig_start
    med = lambda v: f"{statistics.median(v):.0f}"
    print(f"Cold start and bootstrap (BENCH1) · app cores {bm2.APP_CORES} ({bm2.topology(bm2.APP_CORES)}) · {a.runs} starts per case, median (min–max) · "
          f"empty route `/api/bench/sql/health`; first request `{a.endpoint}` against the median of the next {a.warm}\n")
    print("| Target | Workers | Laravel caches | Start to first 200 ms | First request ms | Warm ms |")
    print("|---|---:|---|---:|---:|---:|")
    for name, r in res.items():
        for key, starts in r["start"].items():
            w, caches = key.split("|")
            cache_label = {"keep": "built", "none": "none", "-": "–"}[caches]
            print(f"| {name} | {w} | {cache_label} | {med(starts)} ({min(starts):.0f}–{max(starts):.0f}) | {statistics.median(r['first'][key]):.2f} | {statistics.median(r['warm'][key]):.2f} |")
    print("\n| Target | Hot reload | To the new answer ms (median of 3) |")
    print("|---|---|---:|")
    for name, r in res.items():
        if not r["reload"]:
            continue
        print(f"| {name} | {r['reload']['how']} | {med(r['reload']['ms'])} ({min(r['reload']['ms']):.0f}–{max(r['reload']['ms']):.0f}) |")
    print()
    if a.json:
        json.dump(res, open(a.json, "w"), indent=1)


if __name__ == "__main__":
    main()
