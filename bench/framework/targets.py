#!/usr/bin/env python3
"""The frameworks of the benchmark and their targets (BENCH3): which target
belongs to which framework, its label, its server variant, and which variant
is the framework's fastest in a bm2 result. One place for bm2.py, the BENCH1
tools, report.py, plan.py and run_all.sh.

  targets.py list [stage]            frameworks of a stage (bm2: all; thesis, load, memory, coldstart)
  targets.py of <fw>                 the bm2 targets of a framework, comma-separated
  targets.py best <bm2.json> <fw> <stage>
                                     the targets of <fw> for a later stage: every
                                     variant for bm2, else the fastest one in bm2.json
                                     (geometric mean of requests/s over all points);
                                     OpenGrid and Laravel keep their fixed sets
Target names: opengrid-…-core and laravel-…-core as before (comparable with
70ba137), the comparison apps <app>[-<variant>]-core, started through
bench/frameworks/serve.sh <app> <port> <dir> <workers> <variant>.
"""

import json, math, sys

# framework: (label, [(target, variant label, serve.sh app, serve.sh variant)])
FRAMEWORKS = {
    "opengrid": ("OpenGrid", [("opengrid-embedded-pgo-core", "PGO", None, None), ("opengrid-embedded-core", "without PGO", None, None)]),
    "laravel": ("Laravel", [("laravel-frankenphp-core", "Octane, FrankenPHP", None, None), ("laravel-octane-core", "Octane, RoadRunner", None, None), ("laravel-fpm-core", "PHP-FPM", None, None)]),
    "adonis": ("AdonisJS", [("adonis-core", "Node cluster", "adonis", "")]),
    "nest": ("NestJS", [("nest-express-core", "Express", "nest", "express"), ("nest-fastify-core", "Fastify", "nest", "fastify")]),
    "next": ("Next.js", [("next-core", "next start, cluster", "next", "")]),
    "rails": ("Rails", [("rails-threads3-core", "Puma, 3 threads", "rails", "threads3"), ("rails-threads1-core", "Puma, 1 thread", "rails", "threads1")]),
    "django": ("Django", [("django-gunicorn-core", "Gunicorn", "django", "gunicorn"), ("django-granian-core", "Granian", "django", "granian")]),
    "fastapi": ("FastAPI", [("fastapi-uvicorn-core", "Uvicorn", "fastapi", "uvicorn"), ("fastapi-granian-core", "Granian", "fastapi", "granian")]),
    "spring": ("Spring Boot", [("spring-platform-core", "platform threads", "spring", "platform"), ("spring-vthreads-core", "virtual threads", "spring", "vthreads")]),
    "dotnet": ("ASP.NET Core", [("dotnet-core", "Kestrel", "dotnet", "")]),
    "phoenix": ("Phoenix", [("phoenix-core", "Bandit, release", "phoenix", "")]),
}
# Spring Boot on the JVM: the upper reference for a mature JIT (Jonas, BENCH3)
JIT_REFERENCE = "spring"
# the stages per framework; OpenGrid and Laravel keep the target sets of
# 70ba137 there, the others measure their fastest variant from bm2
FIXED = {
    "thesis": {"opengrid": ["opengrid-embedded-core", "opengrid-embedded-pgo-core"], "laravel": ["laravel-frankenphp-core", "laravel-octane-core", "laravel-fpm-core"]},
    "load": {"opengrid": ["opengrid-embedded-pgo-core"], "laravel": ["laravel-frankenphp-core", "laravel-octane-core", "laravel-fpm-core"]},
    "memory": {"opengrid": ["opengrid-embedded-pgo-core"], "laravel": ["laravel-frankenphp-core", "laravel-octane-core", "laravel-fpm-core"]},
    "coldstart": {"opengrid": ["opengrid-embedded-pgo-core"], "laravel": ["laravel-frankenphp-core", "laravel-octane-core", "laravel-fpm-core"]},
}
STAGES = ["bm2", "thesis", "load", "memory", "coldstart"]


def framework_of(target):
    for fw, (_, ts) in FRAMEWORKS.items():
        if any(t == target for t, *_ in ts):
            return fw
    return None


def info(target):
    """(framework, framework label, variant label, serve.sh app, serve.sh variant) or None."""
    for fw, (label, ts) in FRAMEWORKS.items():
        for t, v, app, var in ts:
            if t == target:
                return fw, label, v, app, var
    return None


def label(target):
    i = info(target)
    return f"{i[1]} ({i[2]})" if i else target


def geomean(values):
    values = [v for v in values if v > 0]
    return math.exp(sum(math.log(v) for v in values) / len(values)) if values else 0.0


def score(bm2, target):
    return geomean([v[0] for k, v in bm2.items() if k.count("|") == 2 and k.split("|")[1] == target])


def fastest(bm2, fw):
    """The variant of fw with the highest geometric mean of requests/s in bm2."""
    ts = [t for t, *_ in FRAMEWORKS[fw][1]]
    return max(ts, key=lambda t: score(bm2, t))


def best(bm2, fw, stage):
    if stage == "bm2":
        return [t for t, *_ in FRAMEWORKS[fw][1]]
    if fw in FIXED.get(stage, {}):
        return FIXED[stage][fw]
    return [fastest(bm2, fw)]


def main():
    cmd = sys.argv[1:] or ["list"]
    if cmd[0] == "list":
        print(" ".join(FRAMEWORKS))
    elif cmd[0] == "of":
        print(",".join(t for t, *_ in FRAMEWORKS[cmd[1]][1]))
    elif cmd[0] == "best":
        fixed = cmd[3] == "bm2" or cmd[2] in FIXED.get(cmd[3], {})
        bm2 = {} if fixed else json.load(open(cmd[1]))
        print(",".join(best(bm2, cmd[2], cmd[3])))
    else:
        sys.exit(__doc__)


if __name__ == "__main__":
    main()
