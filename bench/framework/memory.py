#!/usr/bin/env python3
"""Memory of a whole stack (BENCH1): PSS (proportional set size: shared
pages split between the processes that share them, so a stack's processes
add up) over every process of a target, from /proc/<pid>/smaps_rollup.
Targets are set up as by bm2.py (same cores, workers, data).

Processes per target:
- app: the server process and all its descendants (OpenGrid; nginx +
  PHP-FPM master and children; Octane's PHP master, RoadRunner and its PHP
  workers; FrankenPHP), including the start scripts;
- database: SQLite lives inside the app process on both sides (and
  OpenGrid's embedded SurrealDB, opened at start for the job poller, D25),
  so app + database = app.

Phases: idle (after setup and seeding), under load (oha at --conc
connections on --endpoint for --duration s, sampled every 0.25 s: peak and
mean), after 10 000 requests (sent at --conc connections, then 2 s idle).

  memory.py [--targets …] [--endpoint show] [--conc 16] [--duration 10] [--requests 10000] [--json FILE]
"""

import argparse, json, os, statistics, subprocess, sys, tempfile, threading, time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import bm2  # noqa: E402

TARGETS = ["opengrid-embedded-pgo-core", "laravel-frankenphp-core"]  # D25: core only


def children():
    kids = {}
    for pid in os.listdir("/proc"):
        if pid.isdigit():
            try:
                st = open(f"/proc/{pid}/stat").read()
                ppid = int(st[st.rindex(")") + 2:].split()[1])
                kids.setdefault(ppid, []).append(int(pid))
            except (OSError, ValueError):
                pass
    return kids


def tree(root, kids):
    out, todo = [], [root]
    while todo:
        p = todo.pop()
        out.append(p)
        todo += kids.get(p, [])
    return out


def pss_kib(pids):
    total = 0
    for p in pids:
        try:
            for line in open(f"/proc/{p}/smaps_rollup"):
                if line.startswith("Pss:"):
                    total += int(line.split()[1])
                    break
        except OSError:
            pass
    return total


def db_root(t):
    return None


def sample(t):
    kids = children()
    app = pss_kib(tree(t.procs[-1].pid, kids))
    d = db_root(t)
    db = pss_kib(tree(d, kids)) if d else 0
    return app, db


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--targets", default=",".join(TARGETS))
    ap.add_argument("--endpoint", default="show")
    ap.add_argument("--conc", type=int, default=16)
    ap.add_argument("--duration", type=int, default=10)
    ap.add_argument("--requests", type=int, default=10000)
    ap.add_argument("--workers", type=int, default=4)
    ap.add_argument("--notes", type=int, default=200)
    ap.add_argument("--opengrid", default=os.path.join(bm2.WS, "target/perf/opengrid"))
    ap.add_argument("--opengrid-pgo", default=os.path.join(bm2.WS, "target-pgo/opengrid-pgo"))
    ap.add_argument("--json", help="write the raw results here")
    a = ap.parse_args()
    a.app_cores, a.db_cores, a.load_cores = bm2.APP_CORES, bm2.DB_CORES, bm2.LOAD_CORES
    a.opengrid, a.opengrid_pgo = os.path.abspath(a.opengrid), os.path.abspath(a.opengrid_pgo)
    res = {}
    for name in a.targets.split(","):
        with tempfile.TemporaryDirectory(prefix=f"mem-{name}-") as work:
            t = bm2.Target(name, a, work)
            try:
                t.launch()
                t.seed()
                time.sleep(2)
                idle = sample(t)
                url, method, body, token = t.endpoint(a.endpoint)
                samples, stop = [], []

                def sampler():
                    while not stop:
                        samples.append(sample(t))
                        time.sleep(0.25)
                th = threading.Thread(target=sampler)
                th.start()
                bm2.oha(url, a.conc, a.duration, a.load_cores, method, body, token)
                stop.append(1)
                th.join()
                cmd = ["taskset", "-c", a.load_cores, "oha", "--no-tui", "--output-format", "json", "-n", str(a.requests), "-c", str(a.conc), "-m", method]
                if token:
                    cmd += ["-H", f"Authorization: Bearer {token}"]
                if body is not None:
                    cmd += ["-T", "application/json", "-d", json.dumps(body)]
                subprocess.run(cmd + [url], capture_output=True, check=True)
                time.sleep(2)
                after = sample(t)
                both = [s[0] + s[1] for s in samples]
                res[name] = {"idle": idle, "after": after, "load_app_peak": max(s[0] for s in samples), "load_app_mean": statistics.mean(s[0] for s in samples),
                             "load_all_peak": max(both), "load_all_mean": statistics.mean(both), "db_separate": db_root(t) is not None}
                r = res[name]
                print(f"  {name:<26} idle {idle[0] / 1024:.0f}+{idle[1] / 1024:.0f} MiB, load peak {r['load_app_peak'] / 1024:.0f} MiB (app), after {after[0] / 1024:.0f}+{after[1] / 1024:.0f} MiB", file=sys.stderr)
            finally:
                t.close()
    mib = lambda k: f"{k / 1024:.1f}"
    print(f"Memory (PSS from /proc/<pid>/smaps_rollup, every process of a stack, MiB) · {a.workers} workers per side · load: `{a.endpoint}` with {a.conc} connections, {a.duration} s, "
          f"sampled every 0.25 s · then {a.requests} requests and 2 s idle\n")
    print("| Target | App: idle | App: load peak | App: load mean | App: after 10k | App + DB: idle | App + DB: load peak | App + DB: load mean | App + DB: after 10k |")
    print("|---|---:|---:|---:|---:|---:|---:|---:|---:|")
    for name, r in res.items():
        emb = "" if r["db_separate"] else " (DB eingebettet)"
        print(f"| {name}{emb} | {mib(r['idle'][0])} | {mib(r['load_app_peak'])} | {mib(r['load_app_mean'])} | {mib(r['after'][0])} | "
              f"{mib(sum(r['idle']))} | {mib(r['load_all_peak'])} | {mib(r['load_all_mean'])} | {mib(sum(r['after']))} |")
    print("\nSQLite runs inside the app process on every side (app + DB = app); OpenGrid additionally contains the embedded SurrealDB it opens at start (job poller, D25).\n")
    if a.json:
        json.dump(res, open(a.json, "w"), indent=1)


if __name__ == "__main__":
    main()
