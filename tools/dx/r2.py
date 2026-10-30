#!/usr/bin/env python3
"""R2: the PHP code blocks of the relevant Laravel docs chapters, classified
for OpenGrid (D12 Phase 1, REFERENCES.md "laravel-docs").

Reads refs/framework/laravel-docs at run time (nothing is copied). Per block:
- dialect: compiled with `grd compile`; GRD codes found, clean, or a fragment
  (the parser does not accept it on its own, e.g. a method without class);
- API: the Laravel APIs it calls (Str::x, Arr::x, collection methods, helper
  functions, Route::x, request/response methods, query builder and model
  methods, validation rule names) compared with what OpenGrid provides (read
  from the preludes and the native class registrations in the sources);
- scope: blocks using facades or features outside OpenGrid's scope.
Verdict per block: out of scope, API missing, dialect change, or runs as is.

usage: tools/dx/r2.py [--grd BIN]   (Markdown on stdout, details in target/refs/r2.json)
"""
import argparse, collections, json, os, re, subprocess, tempfile

REPO = os.path.abspath(os.path.join(os.path.dirname(__file__), "../.."))
WS = os.path.dirname(REPO)
DOCS = os.path.join(WS, "refs/framework/laravel-docs")
CHAPTERS = ["routing", "controllers", "requests", "validation", "eloquent", "eloquent-relationships", "queries",
            "middleware", "responses", "collections", "helpers", "strings", "configuration"]
# facades/features outside OpenGrid's scope (REFERENCES.md: not relevant), or planned later
OUT_OF_SCOPE = {
    r"\bBlade::|@(if|foreach|csrf|extends|section|yield)\b|\bview\(|\bView::": "Views/Blade",
    r"\bCache::|\bcache\(": "Cache",
    r"\bMail::|\bNotification::|->notify\(": "Mail/Notifications",
    r"\bQueue::|\bBus::|dispatch\(|::dispatch": "Queues/Jobs (J2)",
    r"\bEvent::|\bevent\(|Broadcast": "Events/Broadcasting",
    r"\bGate::|->can\(|\bPolicy\b|authorize\(": "Authorization (Gates/Policies)",
    r"\bStorage::|->store\(|->file\(|UploadedFile": "Storage/Uploads (MS1)",
    r"\bHttp::": "HTTP client",
    r"\bArtisan::|\$this->command|Command\b": "Artisan",
    r"\bSchema::|Blueprint|\$table->": "Migrations (schema is declarative)",
    r"\bRateLimiter::|throttle": "Rate limiting (AU2)",
    r"\bLang::|__\(|trans\(": "Localization",
    r"\bCarbon\b|now\(\)->|today\(": "Carbon",
    r"\bredirect\(|->back\(|Redirect::|->withErrors|->withInput": "Redirects/Sessions with views",
    r"\bHash::|bcrypt\(|\bCrypt::|encrypt\(": "Hashing/Encryption facades",
    r"\bLazyCollection\b|->lazy\(|->cursor\(": "LazyCollection (S1b)",
    r"\bStr::of\(|\bstr\(|Stringable": "Fluent strings (S1b)",
}


def sh(cmd, **kw):
    return subprocess.run(cmd, capture_output=True, text=True, **kw).stdout


def blocks(chapter):
    text = open(os.path.join(DOCS, chapter + ".md"), encoding="utf-8").read()
    out, cur, start = [], None, 0
    for i, line in enumerate(text.split("\n")):
        t = line.strip()
        if cur is None and t.startswith("```php"):
            cur, start = [], i + 1
        elif cur is not None and t.startswith("```"):
            out.append((start, "\n".join(cur)))
            cur = None
        elif cur is not None:
            cur.append(line)
    return out


def known_api():
    """What OpenGrid provides, read from its sources."""
    api = collections.defaultdict(set)
    sup = open(os.path.join(REPO, "crates/vm/src/vm/prelude_support.grd")).read()
    for cls, body in re.findall(r"(?:final )?class (\w+)[^{]*\{(.*?)\n\}", sup, re.S):
        api[cls] |= {m.lower() for m in re.findall(r"public (?:static )?function (\w+)", body)}
    preludes = [os.path.join(REPO, p) for p in ["crates/vm/src/vm/prelude.grd", "crates/vm/src/vm/prelude_support.grd"]] + \
               [os.path.join(REPO, "crates/framework/src", f) for f in os.listdir(os.path.join(REPO, "crates/framework/src")) if f.endswith(".grd")]
    for p in preludes:
        src = open(p).read()
        api["function"] |= {f.lower() for f in re.findall(r"^function (\w+)\(", src, re.M)}
        for cls, body in re.findall(r"(?:final |abstract )?class (\w+)[^{]*\{(.*?)\n\}", src, re.S):
            api[cls] |= {m.lower() for m in re.findall(r"public (?:static )?function (\w+)", body)}
    for root in ["crates/vm/src", "crates/framework/src"]:
        for dp, _, fs in os.walk(os.path.join(REPO, root)):
            for f in fs:
                if not f.endswith(".rs"):
                    continue
                src = open(os.path.join(dp, f)).read()
                api["function"] |= {n.lower() for n in re.findall(r'register_native\("(\w+)"', src)}
                cls = re.search(r'pub const CLASS: &str = "[^"]*\\\\(\w+)"', src)
                if cls:
                    api[cls.group(1)] |= {n.lower() for n in re.findall(r'\.(?:static_)?method\("(\w+)"', src)}
    # validation rule names: the match arms of the rule check
    val = open(os.path.join(REPO, "crates/framework/src/native/validation.rs")).read()
    api["rule"] = set(re.findall(r'^\s+"(\w+)"(?: \| "\w+")* =>', val, re.M)) | {"nullable", "sometimes", "bail"}
    for alt in re.findall(r'^\s+"\w+"((?: \| "\w+")+) =>', val, re.M):
        api["rule"] |= set(re.findall(r'"(\w+)"', alt))
    # models answer builder methods (static forwarding) and the other way round for chains
    api["Model"] |= api["Builder"]
    api["Builder"] |= api["Model"]
    return api


# facades and classes that are not models (their static calls are not builder methods)
NOT_MODELS = {"DB", "App", "Route", "Schema", "Str", "Arr", "Log", "Config", "Validator", "Auth", "Cache", "Event", "Gate",
              "Storage", "Http", "Mail", "Queue", "Bus", "Hash", "Crypt", "Lang", "Carbon", "Rule", "Request", "Response",
              "Collection", "LazyCollection", "Model", "Builder", "Artisan", "Session", "Cookie", "URL", "Redirect", "View"}


def apis_used(chapter, code, own_methods=frozenset()):
    """(receiver, method) pairs a block calls, as far as they can be attributed."""
    used = set()
    for cls in ["Str", "Arr", "Route", "DB", "Response", "Validator", "Auth", "Config", "Log", "Session"]:
        used |= {(cls, m.lower()) for m in re.findall(rf"\b{cls}::(\w+)\(", code)}
    used |= {("Request", m.lower()) for m in re.findall(r"(?:\$request|request\(\))->(\w+)\(", code)}
    used |= {("Response", m.lower()) for m in re.findall(r"response\([^;]*?\)->(\w+)\(", code)}
    if chapter == "collections":
        used |= {("Collection", m.lower()) for m in re.findall(r"->(\w+)\(", code)}
    if chapter in ("eloquent", "eloquent-relationships", "queries"):
        used |= {("Builder", m.lower()) for m in re.findall(r"->(\w+)\(", code)} | \
                {("Builder", m.lower()) for c, m in re.findall(r"\b([A-Z]\w*)::(\w+)\(", code) if c not in NOT_MODELS and not m[0].isupper()}
        # relations and scopes the docs' own models define are user code
        used = {(r, m) for r, m in used if not (r == "Builder" and m in own_methods)}
    if chapter == "routing":
        used |= {("Route", m.lower()) for m in re.findall(r"\)->(\w+)\(", code)}
    if chapter in ("helpers", "configuration"):
        used |= {("function", f.lower()) for f in re.findall(r"(?<![\w>:$\\])([a-z_][a-z0-9_]*)\(", code)
                 if f not in ("function", "fn", "if", "foreach", "for", "while", "switch", "match", "return", "array", "list", "isset", "empty", "echo", "print", "new", "use", "catch", "unset")}
    if chapter == "validation":
        for s in re.findall(r"'((?:[a-z_]+(?::[^'|]*)?\|)*[a-z_]+(?::[^'|]*)?)'", code):
            if re.fullmatch(r"[a-z_]+(?::[^|]*)?(?:\|[a-z_]+(?::[^|]*)?)*", s) and ("|" in s or s in ("required", "string", "email", "integer", "numeric", "array", "boolean", "date")):
                used |= {("rule", r.split(":")[0]) for r in s.split("|")}
    return used


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--grd", default=os.path.join(WS, "target/debug/grd"))
    a = ap.parse_args()
    api = known_api()
    results = []
    with tempfile.TemporaryDirectory() as d:
        files = []
        for ch in CHAPTERS:
            for i, (line, code) in enumerate(blocks(ch)):
                src = code if code.lstrip().startswith("<?php") else "<?php\n" + code
                f = os.path.join(d, f"{ch}--{i}.php")
                open(f, "w").write(src)
                files.append((ch, line, code, f))
        out = sh([a.grd, "compile"] + [f for *_, f in files])
        status = {}
        for l in out.split("\n"):
            m = re.match(r"OK\s+(\S+)", l) or re.match(r"ERR\s+(\S+?):\s(.*)$", l)
            if m:
                status[m.group(1)] = ("OK" if l.startswith("OK") else "ERR", m.group(2) if l.startswith("ERR") else "")
    # methods the examples of a chapter define themselves (relations, scopes, …)
    own = collections.defaultdict(set)
    for ch, line, code, f in files:
        own[ch] |= {m.lower() for m in re.findall(r"function (\w+)\(", code)}
        own[ch] |= {m[5:].lower() for m in own[ch] if m.startswith("scope")}
    for ch, line, code, f in files:
        ok, msg = status.get(f, ("ERR", "no result"))
        codes = sorted(set(re.findall(r"GRD\d{3}", msg)))
        dialect = "clean" if ok == "OK" else ("grd" if codes else "fragment")
        scope = [name for pat, name in OUT_OF_SCOPE.items() if re.search(pat, code)]
        missing = sorted(f"{r}::{m}" if r != "function" and r != "rule" else (f"{m}()" if r == "function" else f"rule {m}")
                         for r, m in apis_used(ch, code, own[ch]) if m not in api.get(r, set()))
        verdict = "out of scope" if scope else "API missing" if missing else "dialect change" if dialect == "grd" else "runs as is"
        results.append({"chapter": ch, "line": line, "dialect": dialect, "codes": codes, "scope": scope, "missing": missing, "verdict": verdict})
    os.makedirs(os.path.join(REPO, "target/refs"), exist_ok=True)
    json.dump(results, open(os.path.join(REPO, "target/refs/r2.json"), "w"), indent=1)
    report(results)


def report(results):
    verdicts = ["runs as is", "dialect change", "API missing", "out of scope"]
    names = {"runs as is": "runs as is", "dialect change": "with a dialect change", "API missing": "API missing", "out of scope": "out of scope"}
    print(f"State: phprs {sh(['git', '-C', REPO, 'rev-parse', '--short', 'HEAD']).strip()}, laravel-docs {sh(['git', '-C', DOCS, 'rev-parse', '--short', 'HEAD']).strip() or '13.x'}; {len(results)} PHP blocks in {len(CHAPTERS)} chapters.\n")
    print("| Chapter | Blocks | " + " | ".join(names[v] for v in verdicts) + " | of which fragments¹ |")
    print("|---|---:|" + "---:|" * (len(verdicts) + 1))
    per = collections.defaultdict(collections.Counter)
    for r in results:
        per[r["chapter"]][r["verdict"]] += 1
        per[r["chapter"]]["fragment"] += r["dialect"] == "fragment"
    tot = collections.Counter()
    for ch in CHAPTERS:
        c = per[ch]
        n = sum(c[v] for v in verdicts)
        tot.update({v: c[v] for v in verdicts + ["fragment"]})
        print(f"| {ch} | {n} | " + " | ".join(str(c[v]) for v in verdicts) + f" | {c['fragment']} |")
    n = sum(tot[v] for v in verdicts)
    print(f"| **Summe** | {n} | " + " | ".join(f"{tot[v]} ({100 * tot[v] / n:.0f} %)" for v in verdicts) + f" | {tot['fragment']} |")
    print("\n¹ Blocks that do not compile on their own (methods without a class, `...`); their APIs are checked anyway.")
    miss = collections.Counter(m for r in results if r["verdict"] == "API missing" for m in r["missing"])
    print("\n### Missing APIs (blocks, most frequent 60)\n\n" + ", ".join(f"`{m}` ({k})" for m, k in miss.most_common(60)))
    scope = collections.Counter(s for r in results for s in r["scope"])
    print("\n### Out of scope (blocks per topic)\n\n" + ", ".join(f"{s} ({k})" for s, k in scope.most_common()))
    codes = collections.Counter(c for r in results for c in r["codes"])
    print("\n### Dialect changes (GRD codes, blocks)\n\n" + (", ".join(f"{c} ({k})" for c, k in codes.most_common()) or "none"))


if __name__ == "__main__":
    main()
