"""Measurement tool (BENCH3): the FastAPI comparison app.

The same routes, raw SQL statements and answers as the Laravel app's
BenchSqlController/BenchLogicController and OpenGrid's benchsql module:
/health, /api/bench/sql/{health,login,notes,notes/{id}} and
/api/bench/logic/{hydrate,collect,validate,json}, behind the same token
check (its use written at most once a second). Synchronous endpoints, as
FastAPI recommends for a synchronous database library; one sqlite3
connection per thread with WAL, synchronous FULL, busy_timeout 5000.
"""

import hashlib
import os
import secrets
import sqlite3
import threading
import time
from typing import Annotated, Optional

import bcrypt
from fastapi import Depends, FastAPI, Request
from fastapi.exceptions import RequestValidationError
from fastapi.responses import ORJSONResponse
from pydantic import BaseModel, Field, StrictStr

# orjson for every answer, as the FastAPI docs recommend for performance (BENCH3)
app = FastAPI(openapi_url=None, docs_url=None, redoc_url=None, default_response_class=ORJSONResponse)
_local = threading.local()


def conn() -> sqlite3.Connection:
    c = getattr(_local, "conn", None)
    if c is None:
        c = sqlite3.connect(os.environ["BENCH_SQLITE_DATABASE"], isolation_level=None, check_same_thread=True)
        c.row_factory = sqlite3.Row
        c.execute("PRAGMA journal_mode = WAL")
        c.execute("PRAGMA synchronous = FULL")
        c.execute("PRAGMA busy_timeout = 5000")
        _local.conn = c
    return c


def now_text() -> str:
    return time.strftime("%Y-%m-%d %H:%M:%S", time.gmtime())


def bench_user(request: Request) -> dict:
    """The token check of the framework-core benchmark (BenchToken)."""
    header = request.headers.get("authorization", "")
    token = header[7:] if header.startswith("Bearer ") else ""
    if token == "":
        raise Unauthenticated()
    c = conn()
    row = c.execute("SELECT t.id AS token_id, t.last_used_at, u.id, u.name, u.email FROM tokens t JOIN users u ON u.id = t.user_id WHERE t.token_hash = ? LIMIT 1", (hashlib.sha256(token.encode()).hexdigest(),)).fetchone()
    if row is None:
        raise Unauthenticated()
    now = int(time.time())
    if now - row["last_used_at"] >= 1:
        c.execute("UPDATE tokens SET last_used_at = ? WHERE id = ?", (now, row["token_id"]))
    return {"id": row["id"], "name": row["name"], "email": row["email"]}


class Unauthenticated(Exception):
    pass


@app.exception_handler(Unauthenticated)
def unauthenticated(_request, _exc):
    return ORJSONResponse({"message": "Unauthenticated."}, status_code=401)


def note(row) -> dict:
    return {"id": row["id"], "title": row["title"], "body": row["body"], "done": row["done"] == 1, "priority": row["priority"], "created_at": row["created_at"], "updated_at": row["updated_at"]}


@app.get("/health")
def health():
    return {"status": "ok"}


@app.get("/api/bench/sql/health")
def sql_health():
    return {"status": "ok"}


@app.get("/api/bench/sql/sqlite")
def sqlite_info():
    """The SQLite this side runs and the runtime, for the header."""
    import sys
    c = conn()
    return {"version": c.execute("select sqlite_version()").fetchone()[0], "compile_options": [r[0] for r in c.execute("pragma compile_options")],
            "runtime": {"python": sys.version.split()[0], "server": os.environ.get("BENCH_SERVER", "uvicorn")}}


@app.post("/api/bench/sql/login")
async def login(request: Request):
    data = await request.json()
    email, password = data.get("email"), data.get("password")
    if not isinstance(email, str) or not isinstance(password, str):
        return ORJSONResponse({"message": "The email and password fields are required."}, status_code=422)
    c = conn()
    row = c.execute("SELECT id, password FROM users WHERE email = ? LIMIT 1", (email,)).fetchone()
    if row is None or not bcrypt.checkpw(password.encode(), row["password"].encode()):
        return ORJSONResponse({"message": "Invalid credentials."}, status_code=401)
    token = secrets.token_hex(32)
    c.execute("INSERT INTO tokens (user_id, token_hash, last_used_at, created_at) VALUES (?, ?, ?, ?)", (row["id"], hashlib.sha256(token.encode()).hexdigest(), int(time.time()), now_text()))
    return {"token": token}


def as_int(value: Optional[str], default: int) -> int:
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


@app.get("/api/bench/sql/notes")
def index(request: Request, user: dict = Depends(bench_user)):
    q = request.query_params
    done = 1 if q.get("done", "false") == "true" else 0
    per_page = as_int(q.get("per_page"), 15)
    page = as_int(q.get("page"), 1)
    c = conn()
    total = c.execute("SELECT count(*) AS total FROM notes WHERE user_id = ? AND done = ? AND deleted_at IS NULL", (user["id"], done)).fetchone()["total"]
    rows = c.execute("SELECT id, title, body, done, priority, created_at, updated_at FROM notes WHERE user_id = ? AND done = ? AND deleted_at IS NULL ORDER BY priority DESC, id ASC LIMIT ? OFFSET ?", (user["id"], done, per_page, (page - 1) * per_page)).fetchall()
    return {"data": [note(r) for r in rows], "total": total, "per_page": per_page, "current_page": page}


@app.get("/api/bench/sql/notes/{note_id}")
def show(note_id: str, user: dict = Depends(bench_user)):
    row = conn().execute("SELECT id, title, body, done, priority, created_at, updated_at FROM notes WHERE id = ? AND user_id = ? AND deleted_at IS NULL LIMIT 1", (as_int(note_id, 0), user["id"])).fetchone()
    if row is None:
        return ORJSONResponse({"message": "Not found."}, status_code=404)
    return {"data": note(row)}


@app.post("/api/bench/sql/notes")
async def store(request: Request, user: dict = Depends(bench_user)):
    data = await request.json()
    title = data.get("title")
    if not isinstance(title, str) or title == "":
        return ORJSONResponse({"message": "The title field is required."}, status_code=422)
    body = data.get("body") if isinstance(data.get("body"), str) else None
    priority = data.get("priority", 3)
    priority = priority if isinstance(priority, int) and not isinstance(priority, bool) else 3
    now = now_text()
    new_id = conn().execute("INSERT INTO notes (user_id, title, body, done, priority, created_at, updated_at) VALUES (?, ?, ?, 0, ?, ?, ?) RETURNING id", (user["id"], title, body, priority, now, now)).fetchone()["id"]
    return ORJSONResponse({"data": {"id": new_id, "title": title, "body": body, "done": False, "priority": priority, "created_at": now, "updated_at": now}}, status_code=201)


# ---- THESIS1: logic per request --------------------------------------------

class UserView:
    def __init__(self, id: int, name: str, email: str):
        self.id, self.name, self.email = id, name, email

    def to_array(self) -> dict:
        return {"id": self.id, "name": self.name, "email": self.email}


class NoteView:
    def __init__(self, id, title, body, done, priority, created_at, user: UserView):
        self.id, self.title, self.body, self.done, self.priority, self.created_at, self.user = id, title, body, done, priority, created_at, user

    @staticmethod
    def from_row(row) -> "NoteView":
        return NoteView(row["id"], row["title"], row["body"], row["done"] == 1, row["priority"], row["created_at"], UserView(row["user_id"], row["user_name"], row["user_email"]))

    def to_array(self) -> dict:
        return {"id": self.id, "title": self.title, "body": self.body, "done": self.done, "priority": self.priority, "created_at": self.created_at, "user": self.user.to_array()}


def logic_n(request: Request) -> int:
    return min(1000, max(0, as_int(request.query_params.get("n"), 1)))


def logic_rows(user: dict, n: int):
    return conn().execute("SELECT n.id, n.title, n.body, n.done, n.priority, n.created_at, u.id AS user_id, u.name AS user_name, u.email AS user_email FROM notes n JOIN users u ON u.id = n.user_id WHERE n.user_id = ? AND n.deleted_at IS NULL ORDER BY n.id LIMIT ?", (user["id"], n)).fetchall()


@app.get("/api/bench/logic/hydrate")
def hydrate(request: Request, user: dict = Depends(bench_user)):
    return {"data": [NoteView.from_row(r).to_array() for r in logic_rows(user, logic_n(request))]}


@app.get("/api/bench/logic/collect")
def collect(request: Request, user: dict = Depends(bench_user)):
    mapped = [{"title": r["title"], "priority": r["priority"], "done": r["done"] == 1, "chars": len(r["title"]) + len(r["body"] or "")} for r in logic_rows(user, logic_n(request))]
    groups: dict[int, list] = {}
    for r in (r for r in mapped if not r["done"]):
        groups.setdefault(r["priority"], []).append(r)
    out = {}
    for key in sorted(groups):
        g = groups[key]
        out[str(key)] = {"count": len(g), "chars": sum(r["chars"] for r in g), "first": sorted(g, key=lambda r: r["title"])[0]["title"]}
    return {"groups": out}


class Item(BaseModel):
    # Laravel's `required` fails on "" too
    title: Annotated[StrictStr, Field(min_length=1, max_length=120)]
    priority: Annotated[int, Field(ge=1, le=5)]
    tags: Optional[list[Annotated[StrictStr, Field(max_length=20)]]] = None


class Items(BaseModel):
    # Laravel's `required` fails on an empty list too
    items: Annotated[list[Item], Field(min_length=1)]


# Pydantic's errors in Laravel's form (keys, wording, order by rule and then
# element, the summary), so every side answers the same
_ORDER = ["items", "title", "priority", "tags", "tag"]


def _laravel(err: dict) -> tuple[str, str, tuple]:
    loc = [p for p in err["loc"] if p != "body"]
    key = ".".join(str(p) for p in loc)
    kind, ctx = err["type"], err.get("ctx", {})
    if len(loc) == 1:
        rank = "items"
    elif len(loc) == 3:
        rank = loc[2]
    else:
        rank = "tag"
    messages = {
        "missing": f"The {key} field is required.",
        "too_short": f"The {key} field is required.",
        "string_type": f"The {key} field must be a string.",
        "string_too_long": f"The {key} field must not be greater than {ctx.get('max_length')} characters.",
        "int_type": f"The {key} field must be an integer.",
        "int_parsing": f"The {key} field must be an integer.",
        "int_from_float": f"The {key} field must be an integer.",
        "greater_than_equal": f"The {key} field must be between 1 and 5.",
        "less_than_equal": f"The {key} field must be between 1 and 5.",
        "list_type": f"The {key} field must be an array.",
    }
    nums = tuple(p for p in loc if isinstance(p, int))
    return key, messages.get(kind, f"The {key} field is invalid."), (_ORDER.index(rank) if rank in _ORDER else 9, nums)


@app.exception_handler(RequestValidationError)
def validation_errors(_request, exc: RequestValidationError):
    found = sorted((_laravel(e) for e in exc.errors()), key=lambda t: t[2])
    errors: dict[str, list[str]] = {}
    for key, message, _ in found:
        errors.setdefault(key, []).append(message)
    first = next(iter(errors.values()))[0]
    more = sum(len(v) for v in errors.values()) - 1
    message = first + (f" (and {more} more {'error' if more == 1 else 'errors'})" if more > 0 else "")
    return ORJSONResponse({"message": message, "errors": errors}, status_code=422)


@app.post("/api/bench/logic/validate")
def validate(payload: Items, user: dict = Depends(bench_user)):
    return {"valid": len(payload.items)}


@app.get("/api/bench/logic/json")
def json_(request: Request, user: dict = Depends(bench_user)):
    n = logic_n(request)
    items = [{"id": i, "name": f"item {i}", "tags": ["a", "b", "c"], "meta": {"created": "2026-10-27", "score": i * 1.5, "flags": {"x": True, "y": False}}} for i in range(n)]
    return {"count": n, "items": items}
