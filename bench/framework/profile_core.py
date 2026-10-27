#!/usr/bin/env python3
"""Profile of one framework-core endpoint (PERF4 a): an OpenGrid -core
target set up as by bm2.py, with 1 worker, under Callgrind (perf is blocked
in the container). Collection is off (no instrumentation) during start and warm-up and on only
while `--requests` requests of the endpoint run one after another; the
result is the Callgrind file plus the top functions by own and inclusive
instructions (callgrind_annotate).

  profile_core.py <endpoint> [--requests 50] [--warmup 20] [--notes 1000]
                  [--opengrid BIN] [--out DIR] [--top 40]
  endpoint: as in bm2.py, e.g. hydrate:1000, collect:100, show
Instruction counts are stable between runs; times are not measured here.
"""

import argparse, os, subprocess, sys, tempfile, time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import bm2  # noqa: E402


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("endpoint")
    ap.add_argument("--requests", type=int, default=50)
    ap.add_argument("--warmup", type=int, default=20)
    ap.add_argument("--notes", type=int, default=1000)
    ap.add_argument("--opengrid", default=os.path.join(bm2.WS, "target/profiling/opengrid"))
    ap.add_argument("--out", default=os.path.join(bm2.WS, "target/profile"))
    ap.add_argument("--top", type=int, default=40)
    args = ap.parse_args()
    os.makedirs(args.out, exist_ok=True)
    out = os.path.join(args.out, f"callgrind.{args.endpoint.replace(':', '-')}.out")
    with tempfile.TemporaryDirectory(prefix="profile-core-") as work:
        # only the server runs under Callgrind; `task migrate_push` runs natively
        wrapper = os.path.join(work, "opengrid-callgrind")
        with open(wrapper, "w") as f:
            f.write(f'#!/bin/sh\nif [ "$1" = start ]; then exec valgrind --tool=callgrind --instr-atstart=no --callgrind-out-file={out} {os.path.abspath(args.opengrid)} "$@"; fi\nexec {os.path.abspath(args.opengrid)} "$@"\n')
        os.chmod(wrapper, 0o755)
        a = argparse.Namespace(workers=1, notes=args.notes, app_cores=bm2.APP_CORES, db_cores=bm2.DB_CORES, load_cores=bm2.LOAD_CORES, opengrid=wrapper, opengrid_pgo=wrapper)
        t = bm2.Target("opengrid-embedded-core", a, work)
        try:
            t.launch()
            t.seed()
            url, method, body, token = t.endpoint(args.endpoint)
            for _ in range(args.warmup):
                st, _ = bm2.http_raw(method, url, body, token)
                assert 200 <= st < 300, st
            pid = str(t.procs[-1].pid)
            subprocess.run(["callgrind_control", "-i", "on", pid], check=True, capture_output=True)
            t0 = time.time()
            for _ in range(args.requests):
                st, _ = bm2.http_raw(method, url, body, token)
                assert 200 <= st < 300, st
            subprocess.run(["callgrind_control", "-i", "off", pid], check=True, capture_output=True)
            subprocess.run(["callgrind_control", "-d", pid], check=True, capture_output=True)
            print(f"{args.requests} requests under Callgrind in {time.time() - t0:.1f} s", file=sys.stderr)
        finally:
            t.close()
    # the dump while running is <out>.1; the file written at exit holds nothing
    dumped = out + ".1"
    if os.path.exists(dumped):
        os.replace(dumped, out)
    for inclusive in ("no", "yes"):
        r = subprocess.run(["callgrind_annotate", f"--inclusive={inclusive}", "--threshold=99", out], capture_output=True, text=True)
        lines = r.stdout.splitlines()
        start = next((i for i, l in enumerate(lines) if l.lstrip().startswith("Ir")), 0)
        print(f"## {'inclusive' if inclusive == 'yes' else 'self'} ({args.endpoint}, {args.requests} requests)\n")
        print("\n".join(lines[start:start + args.top + 3]))
        print()
    print(f"callgrind file: {out}")


if __name__ == "__main__":
    main()
