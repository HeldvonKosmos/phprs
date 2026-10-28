#!/usr/bin/env python3
"""Quiet check during a measurement (BENCH1): samples the machine every
`--interval` seconds until it gets SIGTERM and reports what was not part of
the benchmark.
- foreign processes: any process outside the benchmark's own programs
  (oha, opengrid, grd, php, php-fpm, nginx, rr, frankenphp,
  surreal, the bench scripts, the PGO tools) that used more than
  `--threshold` % of a core in a sample,
- idle cores: the cores no part of the benchmark uses (by default the SMT
  siblings of the application cores) busy above `--threshold` %.
Writes one line per finding to `--log` (for a busy idle core with the
threads that ran there, process/thread and share of a core); at the end prints `quiet: ok` or
`quiet: N findings (see <log>)`.

  bench/quiet.py --log FILE [--interval 2] [--threshold 5] [--idle-cores 9,11,13,15]
"""

import argparse, os, signal, sys, time

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "framework"))
import bm2  # noqa: E402

OWN = {"oha", "opengrid", "opengrid-pgo", "grd", "grd-pgo", "php", "php-fpm", "nginx", "rr", "rr.bin", "frankenphp",
       "surreal", "python3", "bash", "sh", "taskset", "timeout", "valgrind", "flock", "llvm-profdata",
       # the process name is cut to 15 characters
       "grd-worker-0", "grd-worker-1", "grd-worker-2", "grd-worker-3", "tokio-rt-worker"}
TICK = os.sysconf("SC_CLK_TCK")


def procs():
    out = {}
    for pid in os.listdir("/proc"):
        if not pid.isdigit():
            continue
        try:
            st = open(f"/proc/{pid}/stat").read()
            name = st[st.index("(") + 1:st.rindex(")")]
            f = st[st.rindex(")") + 2:].split()
            out[int(pid)] = (name, int(f[11]) + int(f[12]))
        except (OSError, ValueError):
            pass
    return out


def threads():
    """CPU ticks and last CPU of every thread: (pid, tid) -> (process name, thread name, ticks, cpu)."""
    out = {}
    for pid in os.listdir("/proc"):
        if not pid.isdigit():
            continue
        try:
            pname = open(f"/proc/{pid}/comm").read().strip()
            for tid in os.listdir(f"/proc/{pid}/task"):
                st = open(f"/proc/{pid}/task/{tid}/stat").read()
                f = st[st.rindex(")") + 2:].split()
                out[(pid, tid)] = (pname, st[st.index("(") + 1:st.rindex(")")], int(f[11]) + int(f[12]), int(f[36]))
        except (OSError, ValueError, IndexError):
            pass
    return out


def cpu_times():
    t = {}
    for line in open("/proc/stat"):
        if line.startswith("cpu") and line[3].isdigit():
            f = line.split()
            vals = list(map(int, f[1:]))
            t[int(f[0][3:])] = (sum(vals) - vals[3] - vals[4], sum(vals))
    return t


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--log", required=True)
    ap.add_argument("--interval", type=float, default=2)
    ap.add_argument("--threshold", type=float, default=5)
    sib = sorted({int(x) for c in bm2.cpus(bm2.APP_CORES) for x in bm2.cpus(open(f"/sys/devices/system/cpu/cpu{c}/topology/thread_siblings_list").read().strip())} - set(bm2.cpus(bm2.APP_CORES)))
    ap.add_argument("--idle-cores", default=",".join(map(str, sib)))
    a = ap.parse_args()
    idle = bm2.cpus(a.idle_cores) if a.idle_cores else []
    stop = []
    signal.signal(signal.SIGTERM, lambda *_: stop.append(1))
    signal.signal(signal.SIGINT, lambda *_: stop.append(1))
    me = os.getpid()
    findings = 0
    log = open(a.log, "a", buffering=1)
    p0, c0, h0, t0 = procs(), cpu_times(), threads(), time.monotonic()
    while not stop:
        time.sleep(a.interval)
        p1, c1, h1, t1 = procs(), cpu_times(), threads(), time.monotonic()
        dt = t1 - t0
        stamp = time.strftime("%H:%M:%S")
        for pid, (name, ticks) in p1.items():
            if pid == me or name in OWN or name.startswith(("grd-worker", "php-fpm", "surreal", "tokio")):
                continue
            used = 100 * (ticks - p0.get(pid, (name, ticks))[1]) / TICK / dt
            if used > a.threshold:
                findings += 1
                log.write(f"{stamp} process {pid} {name}: {used:.0f} % of a core\n")
        for c in idle:
            if c in c0 and c in c1 and c1[c][1] > c0[c][1]:
                busy = 100 * (c1[c][0] - c0[c][0]) / (c1[c][1] - c0[c][1])
                if busy > a.threshold:
                    findings += 1
                    # who ran there: threads whose last CPU is this core and that used CPU in the sample
                    who = sorted(((v[2] - h0[k][2], f"{v[0]}/{v[1]}") for k, v in h1.items() if v[3] == c and k in h0 and v[2] > h0[k][2]), reverse=True)[:3]
                    names = ", ".join(f"{n} {100 * t / TICK / dt:.0f} %" for t, n in who) or "no thread found (interrupts or short-lived processes)"
                    log.write(f"{stamp} idle core {c}: {busy:.0f} % busy ({names})\n")
        p0, c0, h0, t0 = p1, c1, h1, t1
    print(f"quiet: ok (idle cores {a.idle_cores or '-'}, threshold {a.threshold:g} %)" if findings == 0 else f"quiet: {findings} findings (see {a.log})")


if __name__ == "__main__":
    main()
