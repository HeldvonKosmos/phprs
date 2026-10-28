// Measurement tool (BENCH3): the ASP.NET Core comparison app (Minimal APIs on
// Kestrel). The same routes, raw SQL statements and answers as the Laravel
// app's BenchSqlController/BenchLogicController and OpenGrid's benchsql
// module, behind the same token check (its use written at most once a
// second). Raw SQL through Microsoft.Data.Sqlite on the system's libsqlite3
// (the benchmark's SQLite 3.53.4), WAL, synchronous FULL, busy_timeout 5000.
using System.ComponentModel.DataAnnotations;
using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
using Microsoft.Data.Sqlite;

SQLitePCL.raw.SetProvider(new SQLitePCL.SQLite3Provider_sqlite3());

var builder = WebApplication.CreateSlimBuilder(args);
var app = builder.Build();
var dbPath = Environment.GetEnvironmentVariable("BENCH_SQLITE_DATABASE") ?? "bench.sqlite";
var connString = new SqliteConnectionStringBuilder { DataSource = dbPath, Pooling = false, DefaultTimeout = 5 }.ToString();

// one connection per thread, the pragmas set once (as on the other sides)
var connections = new ThreadLocal<SqliteConnection>(() =>
{
    var c = new SqliteConnection(connString);
    c.Open();
    using var cmd = c.CreateCommand();
    cmd.CommandText = "PRAGMA journal_mode = WAL; PRAGMA synchronous = FULL; PRAGMA busy_timeout = 5000;";
    cmd.ExecuteNonQuery();
    return c;
});

List<Dictionary<string, object?>> Rows(string sql, params object?[] args)
{
    var c = connections.Value!;
    using var cmd = c.CreateCommand();
    cmd.CommandText = sql;
    for (var i = 0; i < args.Length; i++) cmd.Parameters.AddWithValue("$p" + (i + 1), args[i] ?? DBNull.Value);
    using var r = cmd.ExecuteReader();
    var rows = new List<Dictionary<string, object?>>();
    while (r.Read())
    {
        var row = new Dictionary<string, object?>(r.FieldCount);
        for (var i = 0; i < r.FieldCount; i++) row[r.GetName(i)] = r.IsDBNull(i) ? null : r.GetValue(i);
        rows.Add(row);
    }
    return rows;
}

static string P(string sql)
{
    // `?` placeholders as $p1, $p2, … for Microsoft.Data.Sqlite
    var sb = new StringBuilder();
    var n = 0;
    foreach (var ch in sql) sb.Append(ch == '?' ? "$p" + ++n : ch.ToString());
    return sb.ToString();
}

static string Sha256(string s) => Convert.ToHexStringLower(SHA256.HashData(Encoding.UTF8.GetBytes(s)));
static string Now() => DateTime.UtcNow.ToString("yyyy-MM-dd HH:mm:ss");
static long L(object? v) => Convert.ToInt64(v);

// PHP's (int) cast of a query value: leading digits, else 0
static int AsInt(string? value, int fallback)
{
    var m = System.Text.RegularExpressions.Regex.Match((value ?? fallback.ToString()).Trim(), @"^[+-]?\d+");
    return m.Success && int.TryParse(m.Value, out var v) ? v : 0;
}

static object Note(Dictionary<string, object?> r) => new { id = L(r["id"]), title = (string?)r["title"], body = (string?)r["body"], done = L(r["done"]) == 1, priority = L(r["priority"]), created_at = (string?)r["created_at"], updated_at = (string?)r["updated_at"] };

async Task<JsonElement> Body(HttpRequest req)
{
    try { return (await JsonDocument.ParseAsync(req.Body)).RootElement.Clone(); }
    catch (JsonException) { return default; }
}

// the token check of the framework-core benchmark (BenchToken) as an endpoint filter
var benchToken = new Func<EndpointFilterInvocationContext, EndpointFilterDelegate, ValueTask<object?>>(async (ctx, next) =>
{
    var header = ctx.HttpContext.Request.Headers.Authorization.ToString();
    var token = header.StartsWith("Bearer ") ? header[7..] : "";
    if (token == "") return Results.Json(new { message = "Unauthenticated." }, statusCode: 401);
    var rows = Rows(P("SELECT t.id AS token_id, t.last_used_at, u.id, u.name, u.email FROM tokens t JOIN users u ON u.id = t.user_id WHERE t.token_hash = ? LIMIT 1"), Sha256(token));
    if (rows.Count == 0) return Results.Json(new { message = "Unauthenticated." }, statusCode: 401);
    var row = rows[0];
    var now = DateTimeOffset.UtcNow.ToUnixTimeSeconds();
    if (now - L(row["last_used_at"]) >= 1) Rows(P("UPDATE tokens SET last_used_at = ? WHERE id = ?"), now, row["token_id"]);
    ctx.HttpContext.Items["benchUser"] = (Id: L(row["id"]), Name: (string?)row["name"], Email: (string?)row["email"]);
    return await next(ctx);
});
static (long Id, string? Name, string? Email) User(HttpContext c) => ((long, string?, string?))c.Items["benchUser"]!;

app.MapGet("/health", () => Results.Json(new { status = "ok" }));
app.MapGet("/api/bench/sql/health", () => Results.Json(new { status = "ok" }));

// the SQLite this side runs and the runtime, for the header
app.MapGet("/api/bench/sql/sqlite", () => Results.Json(new
{
    version = (string)Rows("select sqlite_version() AS v")[0]["v"]!,
    compile_options = Rows("pragma compile_options").Select(r => (string)r["compile_options"]!).ToList(),
    runtime = new { dotnet = Environment.Version.ToString(), processor_count = Environment.ProcessorCount, server_gc = System.Runtime.GCSettings.IsServerGC },
}));

app.MapPost("/api/bench/sql/login", async (HttpRequest req) =>
{
    var b = await Body(req);
    if (b.ValueKind != JsonValueKind.Object || !b.TryGetProperty("email", out var email) || email.ValueKind != JsonValueKind.String || !b.TryGetProperty("password", out var password) || password.ValueKind != JsonValueKind.String)
        return Results.Json(new { message = "The email and password fields are required." }, statusCode: 422);
    var rows = Rows(P("SELECT id, password FROM users WHERE email = ? LIMIT 1"), email.GetString());
    if (rows.Count == 0 || !BCrypt.Net.BCrypt.Verify(password.GetString(), (string)rows[0]["password"]!)) return Results.Json(new { message = "Invalid credentials." }, statusCode: 401);
    var token = Convert.ToHexStringLower(RandomNumberGenerator.GetBytes(32));
    Rows(P("INSERT INTO tokens (user_id, token_hash, last_used_at, created_at) VALUES (?, ?, ?, ?)"), rows[0]["id"], Sha256(token), DateTimeOffset.UtcNow.ToUnixTimeSeconds(), Now());
    return Results.Json(new { token });
});

var notes = app.MapGroup("/api/bench/sql/notes").AddEndpointFilter(benchToken);
// GET /api/bench/sql/notes?done=false&per_page=15&page=1
notes.MapGet("", (HttpContext c) =>
{
    var q = c.Request.Query;
    var done = (q["done"].FirstOrDefault() ?? "false") == "true" ? 1 : 0;
    var perPage = AsInt(q["per_page"].FirstOrDefault(), 15);
    var page = AsInt(q["page"].FirstOrDefault(), 1);
    var u = User(c);
    var total = L(Rows(P("SELECT count(*) AS total FROM notes WHERE user_id = ? AND done = ? AND deleted_at IS NULL"), u.Id, done)[0]["total"]);
    var rows = Rows(P("SELECT id, title, body, done, priority, created_at, updated_at FROM notes WHERE user_id = ? AND done = ? AND deleted_at IS NULL ORDER BY priority DESC, id ASC LIMIT ? OFFSET ?"), u.Id, done, perPage, (page - 1) * perPage);
    return Results.Json(new { data = rows.Select(Note).ToList(), total, per_page = perPage, current_page = page });
});
notes.MapGet("/{id}", (HttpContext c, string id) =>
{
    var rows = Rows(P("SELECT id, title, body, done, priority, created_at, updated_at FROM notes WHERE id = ? AND user_id = ? AND deleted_at IS NULL LIMIT 1"), AsInt(id, 0), User(c).Id);
    return rows.Count == 0 ? Results.Json(new { message = "Not found." }, statusCode: 404) : Results.Json(new { data = Note(rows[0]) });
});
notes.MapPost("", async (HttpContext c) =>
{
    var b = await Body(c.Request);
    if (b.ValueKind != JsonValueKind.Object || !b.TryGetProperty("title", out var title) || title.ValueKind != JsonValueKind.String || title.GetString() == "")
        return Results.Json(new { message = "The title field is required." }, statusCode: 422);
    string? text = b.TryGetProperty("body", out var bb) && bb.ValueKind == JsonValueKind.String ? bb.GetString() : null;
    long priority = b.TryGetProperty("priority", out var pr) ? (pr.ValueKind == JsonValueKind.Number && pr.TryGetInt64(out var pv) ? pv : 3) : 3;
    var t = Now();
    var id = L(Rows(P("INSERT INTO notes (user_id, title, body, done, priority, created_at, updated_at) VALUES (?, ?, ?, 0, ?, ?, ?) RETURNING id"), User(c).Id, title.GetString(), text, priority, t, t)[0]["id"]);
    return Results.Json(new { data = new { id, title = title.GetString(), body = text, done = false, priority, created_at = t, updated_at = t } }, statusCode: 201);
});

// ---- THESIS1: logic per request --------------------------------------------
var logic = app.MapGroup("/api/bench/logic").AddEndpointFilter(benchToken);
static int LogicN(HttpContext c) => Math.Min(1000, Math.Max(0, AsInt(c.Request.Query["n"].FirstOrDefault(), 1)));
List<Dictionary<string, object?>> LogicRows(HttpContext c, int n) =>
    Rows(P("SELECT n.id, n.title, n.body, n.done, n.priority, n.created_at, u.id AS user_id, u.name AS user_name, u.email AS user_email FROM notes n JOIN users u ON u.id = n.user_id WHERE n.user_id = ? AND n.deleted_at IS NULL ORDER BY n.id LIMIT ?"), User(c).Id, n);

// n notes with their user, hydrated into objects and listed
logic.MapGet("/hydrate", (HttpContext c) => Results.Json(new { data = LogicRows(c, LogicN(c)).Select(r => NoteView.FromRow(r).ToArray()).ToList() }));

// n notes prepared with LINQ: Select, Where, GroupBy, OrderBy, Sum
logic.MapGet("/collect", (HttpContext c) =>
{
    var groups = LogicRows(c, LogicN(c))
        .Select(r => new { title = (string)r["title"]!, priority = L(r["priority"]), done = L(r["done"]) == 1, chars = ((string)r["title"]!).Length + ((string?)r["body"] ?? "").Length })
        .Where(r => !r.done)
        .GroupBy(r => r.priority)
        .OrderBy(g => g.Key)
        .ToDictionary(g => g.Key.ToString(), g => (object)new { count = g.Count(), chars = g.Sum(r => (long)r.chars), first = g.OrderBy(r => r.title, StringComparer.Ordinal).First().title });
    return Results.Json(new { groups });
});

// a nested input of n items, validated; answers the number of items. The JSON
// types are checked first (System.Text.Json would reject a wrong type while
// binding, before any validation), the value constraints with DataAnnotations;
// errors in Laravel's form
string[] order = ["items", "title", "priority", "tags", "tag"];
logic.MapPost("/validate", async (HttpContext c) =>
{
    var b = await Body(c.Request);
    var found = new List<(int Rank, int[] Idx, string Key, string Msg)>();
    JsonElement items = default;
    var hasItems = b.ValueKind == JsonValueKind.Object && b.TryGetProperty("items", out items) && items.ValueKind != JsonValueKind.Null;
    if (!hasItems || (items.ValueKind == JsonValueKind.Array && items.GetArrayLength() == 0)) found.Add((0, [], "items", "The items field is required."));
    else if (items.ValueKind != JsonValueKind.Array) found.Add((0, [], "items", "The items field must be an array."));
    else
    {
        var i = 0;
        foreach (var it in items.EnumerateArray())
        {
            var p = "items." + i;
            var item = new Item();
            var typed = new HashSet<string>();
            if (it.ValueKind == JsonValueKind.Object && it.TryGetProperty("title", out var title) && title.ValueKind != JsonValueKind.Null)
            {
                if (title.ValueKind == JsonValueKind.String) item.title = title.GetString();
                else { found.Add((1, [i], p + ".title", $"The {p}.title field must be a string.")); typed.Add("title"); }
            }
            if (it.ValueKind == JsonValueKind.Object && it.TryGetProperty("priority", out var pr) && pr.ValueKind != JsonValueKind.Null)
            {
                if (pr.ValueKind == JsonValueKind.Number && pr.TryGetInt32(out var pv)) item.priority = pv;
                else if (pr.ValueKind == JsonValueKind.String && int.TryParse(pr.GetString(), out var ps)) item.priority = ps;
                else { found.Add((2, [i], p + ".priority", $"The {p}.priority field must be an integer.")); typed.Add("priority"); }
            }
            if (it.ValueKind == JsonValueKind.Object && it.TryGetProperty("tags", out var tags) && tags.ValueKind != JsonValueKind.Null)
            {
                if (tags.ValueKind != JsonValueKind.Array) found.Add((3, [i], p + ".tags", $"The {p}.tags field must be an array."));
                else
                {
                    var j = 0;
                    foreach (var tag in tags.EnumerateArray())
                    {
                        if (tag.ValueKind != JsonValueKind.String) found.Add((4, [i, j], $"{p}.tags.{j}", $"The {p}.tags.{j} field must be a string."));
                        else if (tag.GetString()!.Length > 20) found.Add((4, [i, j], $"{p}.tags.{j}", $"The {p}.tags.{j} field must not be greater than 20 characters."));
                        j++;
                    }
                }
            }
            var results = new List<ValidationResult>();
            Validator.TryValidateObject(item, new ValidationContext(item), results, validateAllProperties: true);
            foreach (var r in results)
            {
                var field = r.MemberNames.First();
                if (typed.Contains(field)) continue;
                var key = $"{p}.{field}";
                if (field == "title") found.Add((1, [i], key, r.ErrorMessage == "required" ? $"The {key} field is required." : $"The {key} field must not be greater than 120 characters."));
                else if (field == "priority") found.Add((2, [i], key, r.ErrorMessage == "required" ? $"The {key} field is required." : $"The {key} field must be between 1 and 5."));
            }
            i++;
        }
    }
    if (found.Count == 0) return Results.Json(new { valid = items.GetArrayLength() });
    var sorted = found.OrderBy(f => f.Rank).ThenBy(f => f.Idx, Comparer<int[]>.Create((a, x) => { for (var k = 0; k < Math.Max(a.Length, x.Length); k++) { var d = (k < a.Length ? a[k] : -1).CompareTo(k < x.Length ? x[k] : -1); if (d != 0) return d; } return 0; })).ToList();
    var errors = new Dictionary<string, List<string>>();
    foreach (var f in sorted) { if (!errors.TryGetValue(f.Key, out var l)) errors[f.Key] = l = []; l.Add(f.Msg); }
    var more = sorted.Count - 1;
    return Results.Json(new { message = sorted[0].Msg + (more > 0 ? $" (and {more} more {(more == 1 ? "error" : "errors")})" : ""), errors }, statusCode: 422);
});

// a nested structure of depth 3 with n items, no database
logic.MapGet("/json", (HttpContext c) =>
{
    var n = LogicN(c);
    var items = new List<object>(n);
    for (var i = 0; i < n; i++) items.Add(new { id = i, name = "item " + i, tags = new[] { "a", "b", "c" }, meta = new { created = "2026-10-27", score = i * 1.5, flags = new { x = true, y = false } } });
    return Results.Json(new { count = n, items });
});

app.Run();

/// One element of the validate workload: DataAnnotations, the rules of the Laravel app.
class Item
{
    [Required(ErrorMessage = "required", AllowEmptyStrings = false), StringLength(120)]
    public string? title { get; set; }

    [Required(ErrorMessage = "required"), Range(1, 5)]
    public int? priority { get; set; }
}

/// A note hydrated from a raw row, with its user (THESIS1).
record UserView(long id, string? name, string? email)
{
    public object ToArray() => new { id, name, email };
}

record NoteView(long id, string? title, string? body, bool done, long priority, string? created_at, UserView user)
{
    public static NoteView FromRow(Dictionary<string, object?> r) =>
        new(Convert.ToInt64(r["id"]), (string?)r["title"], (string?)r["body"], Convert.ToInt64(r["done"]) == 1, Convert.ToInt64(r["priority"]), (string?)r["created_at"], new UserView(Convert.ToInt64(r["user_id"]), (string?)r["user_name"], (string?)r["user_email"]));

    public object ToArray() => new { id, title, body, done, priority, created_at, user = user.ToArray() };
}
