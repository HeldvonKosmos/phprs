#!/usr/bin/env python3
"""Start-up time of OpenGrid (PERF4 c, PAR1; later BENCH1): the test project
copied and set up as by bm2.py (embedded database, schema pushed), then
`opengrid start` with N workers, timed from the process start to the first
200 of /health, repeated; median and range per worker count. The server is
pinned to the application cores like in BM2.

  startup.py [--workers 1,4,16] [--runs 5] [--opengrid BIN] [--cores 8,10,12,14]
Also prints how long compiling all .grd files of the project takes alone
(`grd compile`, one process), as the share PAR1 could parallelise.
"""

import argparse, glob, re, os, shutil, statistics, subprocess, sys, tempfile, time, urllib.request

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import bm2  # noqa: E402


def first_200(url, proc, timeout=120):
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        if proc.poll() is not None:
            sys.exit(f"server exited with {proc.returncode}")
        try:
            with urllib.request.urlopen(url, timeout=1) as r:
                if r.status == 200:
                    return
        except Exception:
            pass
        time.sleep(0.002)
    sys.exit("no 200 within the timeout")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--workers", default="1,4,16")
    ap.add_argument("--runs", type=int, default=5)
    ap.add_argument("--opengrid", default=os.path.join(bm2.WS, "target/dist/opengrid"))
    ap.add_argument("--grd", default=os.path.join(bm2.WS, "target/dist/grd"))
    ap.add_argument("--cores", default=bm2.APP_CORES)
    a = ap.parse_args()
    binary = os.path.abspath(a.opengrid)
    print(f"Binary {binary} · cores {a.cores} · {a.runs} starts per worker count · time to the first 200 of /health\n")
    print("| Worker | Median ms | min ms | max ms |")
    print("|---:|---:|---:|---:|")
    with tempfile.TemporaryDirectory(prefix="startup-") as work:
        app = os.path.join(work, "app")
        shutil.copytree(bm2.TEST_APP, app, ignore=shutil.ignore_patterns(".opengrid", "tests"))
        env = dict(os.environ)
        for k in ("OPENGRID_DB_URL", "SURREAL_DB_HOST", "GRD_JIT", "OPENGRID_JIT"):
            env.pop(k, None)
        env["OPENGRID_DB_URL"] = f"surrealkv://{work}/db"
        subprocess.run([binary, "task", "migrate_push"], cwd=app, env=env, check=True, capture_output=True)
        for w in [int(x) for x in a.workers.split(",")]:
            bm2.patch_config(app, w)
            times = []
            for _ in range(a.runs):
                port = bm2.free_port()
                t0 = time.monotonic()
                p = subprocess.Popen(["taskset", "-c", a.cores, binary, "start", "--no-banner", "--binding", "127.0.0.1", "--port", str(port)], cwd=app, env=env,
                                     stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True)
                try:
                    first_200(f"http://127.0.0.1:{port}/health", p)
                    times.append((time.monotonic() - t0) * 1000)
                finally:
                    bm2.stop(p)
            print(f"| {w} | {statistics.median(times):.0f} | {min(times):.0f} | {max(times):.0f} |")
        files = sorted(glob.glob(os.path.join(app, "**", "*.grd"), recursive=True))
        t0 = time.monotonic()
        r = subprocess.run(["taskset", "-c", re.split("[-,]", a.cores)[0], os.path.abspath(a.grd), "compile"] + files, capture_output=True)
        dt = (time.monotonic() - t0) * 1000
        print(f"\nCompiling all {len(files)} .grd files of the project alone (`grd compile`, one process, one core): {dt:.0f} ms (exit {r.returncode})")


if __name__ == "__main__":
    main()
