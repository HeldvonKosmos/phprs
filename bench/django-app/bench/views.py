"""Measurement tool (BENCH3): the Django comparison app.

The same routes, raw SQL statements and answers as the Laravel app's
BenchSqlController/BenchLogicController and OpenGrid's benchsql module,
behind the same token check (its use written at most once a second). Raw
SQL through django.db.connection; validate with a Django REST framework
serializer, its errors in Laravel's form.
"""

import hashlib
import json
import secrets
import time

import bcrypt
from django.db import connection
from django.http import JsonResponse as _JsonResponse
from django.views.decorators.csrf import csrf_exempt
from rest_framework import serializers


def JsonResponse(data, status=200):
    """Compact JSON like every other side (Django's default writes ", " and ": ",
    which makes large answers about 15 % longer)."""
    return _JsonResponse(data, status=status, json_dumps_params={"separators": (",", ":")})


def rows(sql, params):
    with connection.cursor() as c:
        c.execute(sql, params)
        cols = [d[0] for d in c.description] if c.description else []
        return [dict(zip(cols, r)) for r in c.fetchall()]


def execute(sql, params):
    with connection.cursor() as c:
        c.execute(sql, params)


def now_text():
    return time.strftime("%Y-%m-%d %H:%M:%S", time.gmtime())


def as_int(value, default):
    """PHP's (int) cast of a query value: leading digits, else 0."""
    s = (value if value is not None else str(default)).strip()
    digits = ""
    for i, ch in enumerate(s):
        if ch.isdigit() or (i == 0 and ch in "+-"):
            digits += ch
        else:
            break
    try:
        return int(digits)
    except ValueError:
        return 0


def bench_token(view):
    """The token check of the framework-core benchmark (BenchToken)."""
    def wrapped(request, *args, **kwargs):
        header = request.headers.get("Authorization", "")
        token = header[7:] if header.startswith("Bearer ") else ""
        if token == "":
            return JsonResponse({"message": "Unauthenticated."}, status=401)
        found = rows("SELECT t.id AS token_id, t.last_used_at, u.id, u.name, u.email FROM tokens t JOIN users u ON u.id = t.user_id WHERE t.token_hash = %s LIMIT 1", [hashlib.sha256(token.encode()).hexdigest()])
        if not found:
            return JsonResponse({"message": "Unauthenticated."}, status=401)
        row = found[0]
        now = int(time.time())
        if now - row["last_used_at"] >= 1:
            execute("UPDATE tokens SET last_used_at = %s WHERE id = %s", [now, row["token_id"]])
        request.bench_user = {"id": row["id"], "name": row["name"], "email": row["email"]}
        return view(request, *args, **kwargs)
    return wrapped


def body(request):
    try:
        return json.loads(request.body or b"{}")
    except ValueError:
        return {}


def note(r):
    return {"id": r["id"], "title": r["title"], "body": r["body"], "done": r["done"] == 1, "priority": r["priority"], "created_at": r["created_at"], "updated_at": r["updated_at"]}


def health(request):
    return JsonResponse({"status": "ok"})


def sqlite_info(request):
    """The SQLite this side runs and the runtime, for the header."""
    import os
    import sys

    import django
    return JsonResponse({"version": rows("select sqlite_version() AS v", [])[0]["v"], "compile_options": [r["compile_options"] for r in rows("pragma compile_options", [])],
                         "runtime": {"python": sys.version.split()[0], "django": django.get_version(), "server": os.environ.get("BENCH_SERVER", "gunicorn")}})


@csrf_exempt
def login(request):
    data = body(request)
    email, password = data.get("email"), data.get("password")
    if not isinstance(email, str) or not isinstance(password, str):
        return JsonResponse({"message": "The email and password fields are required."}, status=422)
    found = rows("SELECT id, password FROM users WHERE email = %s LIMIT 1", [email])
    if not found or not bcrypt.checkpw(password.encode(), found[0]["password"].encode()):
        return JsonResponse({"message": "Invalid credentials."}, status=401)
    token = secrets.token_hex(32)
    execute("INSERT INTO tokens (user_id, token_hash, last_used_at, created_at) VALUES (%s, %s, %s, %s)", [found[0]["id"], hashlib.sha256(token.encode()).hexdigest(), int(time.time()), now_text()])
    return JsonResponse({"token": token})


@csrf_exempt
@bench_token
def notes(request):
    if request.method == "POST":
        return store(request)
    user = request.bench_user
    done = 1 if request.GET.get("done", "false") == "true" else 0
    per_page = as_int(request.GET.get("per_page"), 15)
    page = as_int(request.GET.get("page"), 1)
    total = rows("SELECT count(*) AS total FROM notes WHERE user_id = %s AND done = %s AND deleted_at IS NULL", [user["id"], done])[0]["total"]
    found = rows("SELECT id, title, body, done, priority, created_at, updated_at FROM notes WHERE user_id = %s AND done = %s AND deleted_at IS NULL ORDER BY priority DESC, id ASC LIMIT %s OFFSET %s", [user["id"], done, per_page, (page - 1) * per_page])
    return JsonResponse({"data": [note(r) for r in found], "total": total, "per_page": per_page, "current_page": page})


def store(request):
    user = request.bench_user
    data = body(request)
    title = data.get("title")
    if not isinstance(title, str) or title == "":
        return JsonResponse({"message": "The title field is required."}, status=422)
    text = data.get("body") if isinstance(data.get("body"), str) else None
    priority = data.get("priority", 3)
    priority = priority if isinstance(priority, int) and not isinstance(priority, bool) else 3
    now = now_text()
    new_id = rows("INSERT INTO notes (user_id, title, body, done, priority, created_at, updated_at) VALUES (%s, %s, %s, 0, %s, %s, %s) RETURNING id", [user["id"], title, text, priority, now, now])[0]["id"]
    return JsonResponse({"data": {"id": new_id, "title": title, "body": text, "done": False, "priority": priority, "created_at": now, "updated_at": now}}, status=201)


@bench_token
def show(request, note_id):
    found = rows("SELECT id, title, body, done, priority, created_at, updated_at FROM notes WHERE id = %s AND user_id = %s AND deleted_at IS NULL LIMIT 1", [as_int(note_id, 0), request.bench_user["id"]])
    if not found:
        return JsonResponse({"message": "Not found."}, status=404)
    return JsonResponse({"data": note(found[0])})


# ---- THESIS1: logic per request --------------------------------------------

class UserView:
    def __init__(self, id, name, email):
        self.id, self.name, self.email = id, name, email

    def to_array(self):
        return {"id": self.id, "name": self.name, "email": self.email}


class NoteView:
    def __init__(self, id, title, text, done, priority, created_at, user):
        self.id, self.title, self.body, self.done, self.priority, self.created_at, self.user = id, title, text, done, priority, created_at, user

    @staticmethod
    def from_row(r):
        return NoteView(r["id"], r["title"], r["body"], r["done"] == 1, r["priority"], r["created_at"], UserView(r["user_id"], r["user_name"], r["user_email"]))

    def to_array(self):
        return {"id": self.id, "title": self.title, "body": self.body, "done": self.done, "priority": self.priority, "created_at": self.created_at, "user": self.user.to_array()}


def logic_n(request):
    return min(1000, max(0, as_int(request.GET.get("n"), 1)))


def logic_rows(request, n):
    return rows("SELECT n.id, n.title, n.body, n.done, n.priority, n.created_at, u.id AS user_id, u.name AS user_name, u.email AS user_email FROM notes n JOIN users u ON u.id = n.user_id WHERE n.user_id = %s AND n.deleted_at IS NULL ORDER BY n.id LIMIT %s", [request.bench_user["id"], n])


@bench_token
def hydrate(request):
    return JsonResponse({"data": [NoteView.from_row(r).to_array() for r in logic_rows(request, logic_n(request))]})


@bench_token
def collect(request):
    mapped = [{"title": r["title"], "priority": r["priority"], "done": r["done"] == 1, "chars": len(r["title"]) + len(r["body"] or "")} for r in logic_rows(request, logic_n(request))]
    groups = {}
    for r in (r for r in mapped if not r["done"]):
        groups.setdefault(r["priority"], []).append(r)
    out = {}
    for key in sorted(groups):
        g = groups[key]
        out[str(key)] = {"count": len(g), "chars": sum(r["chars"] for r in g), "first": sorted(g, key=lambda r: r["title"])[0]["title"]}
    return JsonResponse({"groups": out})


class StrictCharField(serializers.CharField):
    """Laravel's `string`: a JSON string only (DRF's CharField turns 7 into "7")."""

    def to_internal_value(self, data):
        if not isinstance(data, str):
            self.fail("invalid")
        return super().to_internal_value(data)


class ItemSerializer(serializers.Serializer):
    title = StrictCharField(max_length=120)
    priority = serializers.IntegerField(min_value=1, max_value=5)
    tags = serializers.ListField(child=StrictCharField(max_length=20, allow_blank=True), required=False)


class ItemsSerializer(serializers.Serializer):
    # Laravel's `required` fails on an empty list too
    items = serializers.ListField(child=ItemSerializer(), allow_empty=False)


ORDER = ["items", "title", "priority", "tags", "tag"]


def laravel_errors(errors):
    """DRF's error tree in Laravel's form: keys, wording, order, summary."""
    found = []

    def walk(node, path):
        if isinstance(node, dict):
            for k, v in node.items():
                walk(v, path + [k])
        elif isinstance(node, list) and node and not hasattr(node[0], "code"):
            for i, v in enumerate(node):
                if v:
                    walk(v, path + [i])
        elif isinstance(node, list):
            err = node[0]
            key = ".".join(str(p) for p in path)
            field = [p for p in path if not isinstance(p, int)][-1]
            rank = "items" if len(path) == 1 else ("tag" if isinstance(path[-1], int) else field)
            code = err.code
            if code in ("required", "blank", "empty", "null"):
                msg = f"The {key} field is required."
            elif code == "max_length":
                limit = 120 if rank == "title" else 20
                msg = f"The {key} field must not be greater than {limit} characters."
            elif code == "invalid" and rank in ("title", "tag"):
                msg = f"The {key} field must be a string."
            elif code in ("invalid", "max_string_length"):
                msg = f"The {key} field must be an integer."
            elif code in ("min_value", "max_value"):
                msg = f"The {key} field must be between 1 and 5."
            elif code == "not_a_list":
                msg = f"The {key} field must be an array."
            else:
                msg = f"The {key} field is invalid."
            found.append((ORDER.index(rank) if rank in ORDER else 9, tuple(p for p in path if isinstance(p, int)), key, msg))

    walk(errors, [])
    found.sort(key=lambda t: (t[0], t[1]))
    out = {}
    for _, _, key, msg in found:
        out.setdefault(key, []).append(msg)
    first = next(iter(out.values()))[0]
    more = sum(len(v) for v in out.values()) - 1
    return {"message": first + (f" (and {more} more {'error' if more == 1 else 'errors'})" if more > 0 else ""), "errors": out}


@csrf_exempt
@bench_token
def validate(request):
    s = ItemsSerializer(data=body(request))
    if not s.is_valid():
        return JsonResponse(laravel_errors(s.errors), status=422)
    return JsonResponse({"valid": len(s.validated_data["items"])})


@bench_token
def json_(request):
    n = logic_n(request)
    items = [{"id": i, "name": f"item {i}", "tags": ["a", "b", "c"], "meta": {"created": "2026-10-27", "score": i * 1.5, "flags": {"x": True, "y": False}}} for i in range(n)]
    return JsonResponse({"count": n, "items": items})
