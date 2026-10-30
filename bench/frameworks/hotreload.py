#!/usr/bin/env python3
"""Hot reload of the comparison apps (BENCH3), each in its framework's own
development mode (serve.sh variant dev: Adonis `bun --watch`, Goravel `air`, Rails
development (reloading on the next request), `uvicorn --reload`, Spring Boot devtools with
Gradle's continuous build, `dotnet watch`, Phoenix's code reloader). These
are development modes, not the production setups measured elsewhere, and
are marked so in the report. OpenGrid's and Laravel's reload is measured by
coldstart.py, as before.

Per app: start in development mode, wait for the empty route
/api/bench/sql/health; then --runs times: change its answer in the source
file ("ok" -> "ok<i>") and take the time until the server answers with the
new code (polled every 20 ms); the file is restored and the original answer
awaited before the next change and at the end.

  hotreload.py [--apps adonis,rails,…] [--runs 3] [--timeout 120] [--json FILE]
"""

import argparse, json, os, signal, socket, statistics, subprocess, sys, tempfile, time, urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))
BENCH = os.path.dirname(HERE)
sys.path.insert(0, os.path.join(BENCH, "framework"))
import bm2  # noqa: E402
import targets  # noqa: E402

# app: (source file, the empty route's answer as written there, how the framework reloads)
SOURCES = {
    "adonis": ("adonis-app/app/controllers/bench_sql_controller.ts", "return response.json({ status: 'ok' })", "bun --watch (restart; ace serve --hmr needs Node)"),
    "goravel": ("goravel-app/app/http/controllers/bench_sql_controller.go", 'return ctx.Response().Json(http.StatusOK, http.Json{"status": "ok"})', "air (rebuild and restart, the skeleton's .air.toml)"),
    "rails": ("rails-app/app/controllers/bench_sql_controller.rb", 'render json: { status: "ok" }', "development, reloading per request (Zeitwerk)"),
    "fastapi": ("fastapi-app/app/main.py", 'def sql_health():\n    return {"status": "ok"}', "uvicorn --reload (restart)"),
    "spring": ("spring-app/src/main/java/bench/BenchSqlController.java", 'return new Status("ok");', "devtools + Gradle continuous build (restart)"),
    "dotnet": ("dotnet-app/Program.cs", 'app.MapGet("/api/bench/sql/health", () => Results.Json(new { status = "ok" }));', "dotnet watch (Hot Reload)"),
    "phoenix": ("phoenix-app/lib/phoenix_app_web/controllers/bench_sql_controller.ex", 'def health(conn, _params), do: reply(conn, obj([{"status", "ok"}]))', "mix phx.server (code reloader per request)"),
}


def answer(url):
    try:
        with urllib.request.urlopen(url, timeout=2) as r:
            return r.read()
    except Exception:
        return None


def until(url, proc, want, timeout, log):
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        if proc.poll() is not None:
            tail = open(log, errors="replace").read()[-2000:] if os.path.exists(log) else ""
            sys.exit(f"dev server exited ({proc.returncode})\n{tail}")
        body = answer(url)
        if body is not None and want in body:
            return True
        time.sleep(0.02)
    return False


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--apps", default=",".join(SOURCES))
    ap.add_argument("--runs", type=int, default=3)
    ap.add_argument("--timeout", type=int, default=120)
    ap.add_argument("--json", help="write the raw results here")
    a = ap.parse_args()
    res = {}
    for app in a.apps.split(","):
        rel, old, how = SOURCES[app]
        path = os.path.join(BENCH, rel)
        src = open(path).read()
        assert src.count(old) == 1, f"{path}: the empty route's answer not found once"
        new = lambda mark: src.replace(old, old.replace('"ok"', f'"{mark}"').replace("'ok'", f"'{mark}'"))
        with tempfile.TemporaryDirectory(prefix=f"hot-{app}-") as work:
            port = bm2.free_port()
            url = f"http://127.0.0.1:{port}/api/bench/sql/health"
            db = os.path.join(work, "bench.sqlite")
            subprocess.run(["python3", os.path.join(BENCH, "framework", "sqlite", "seed.py"), db, "4"], check=True, capture_output=True)
            log = os.path.join(work, "dev.log")
            env = dict(os.environ, BENCH_SQLITE_DATABASE=db)
            p = bm2.start(["bash", os.path.join(HERE, "serve.sh"), app, str(port), os.path.join(work, "run"), "4", "dev"], bm2.APP_CORES, env, log)
            times, failed = [], 0
            try:
                t0 = time.monotonic()
                if not until(url, p, b'"ok"', 600, os.path.join(work, "run", "server.log")):
                    sys.exit(f"{app}: the development server did not answer")
                boot = (time.monotonic() - t0) * 1000
                answer(url)
                time.sleep(2)  # let the watchers settle after the first build
                for i in range(1, a.runs + 1):
                    mark = f"ok{i}"
                    open(path, "w").write(new(mark))
                    t0 = time.monotonic()
                    ok = until(url, p, f'"{mark}"'.encode(), a.timeout, os.path.join(work, "run", "server.log"))
                    if ok:
                        times.append((time.monotonic() - t0) * 1000)
                    else:
                        failed += 1
                    open(path, "w").write(src)
                    until(url, p, b'"ok"', a.timeout, os.path.join(work, "run", "server.log"))
                    time.sleep(1)
            finally:
                open(path, "w").write(src)
                bm2.stop(p)
        res[app] = {"ms": times, "failed": failed, "how": how, "boot_ms": boot}
        med = f"{statistics.median(times):.0f} ms" if times else "–"
        print(f"  {app:<8} hot reload ({how}) {med}, {failed} without new answer within {a.timeout} s; dev server up in {boot:.0f} ms", file=sys.stderr)
    print(f"Hot reload in each framework's development mode (BENCH3; **development mode, not the production setup that was measured**) · "
          f"{a.runs} changes of the empty route per app, time until the new answer (polled every 20 ms), median (min–max)\n")
    print("| Framework | Development mode | To the new answer ms | Without new answer | Dev server ready ms |")
    print("|---|---|---:|---:|---:|")
    for app, r in res.items():
        t = r["ms"]
        cell = f"{statistics.median(t):.0f} ({min(t):.0f}–{max(t):.0f})" if t else "–"
        print(f"| {targets.FRAMEWORKS[app][0]} | {r['how']} | {cell} | {r['failed']} | {r['boot_ms']:.0f} |")
    print()
    if a.json:
        json.dump({f"hotreload|{app}": r for app, r in res.items()}, open(a.json, "w"), indent=1)


if __name__ == "__main__":
    main()
