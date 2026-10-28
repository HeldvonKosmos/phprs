package controllers

import (
	"crypto/rand"
	"encoding/hex"
	"os"
	"runtime"
	"runtime/debug"

	"github.com/goravel/framework/contracts/http"
	"github.com/goravel/framework/support"

	"goravel/app/bench"
	"goravel/app/facades"
)

// buildSetting reads a build setting (e.g. GOAMD64) from the binary's build info.
func buildSetting(key string) string {
	if info, ok := debug.ReadBuildInfo(); ok {
		for _, s := range info.Settings {
			if s.Key == key {
				return s.Value
			}
		}
	}
	return ""
}

// BenchSqlController is the framework-core benchmark with raw SQL on
// SQLite (BENCH3). The Laravel app and OpenGrid have the same controller
// with the same statements and the same answers.
type BenchSqlController struct{}

func NewBenchSqlController() *BenchSqlController {
	return &BenchSqlController{}
}

func (r *BenchSqlController) Health(ctx http.Context) http.Response {
	return ctx.Response().Json(http.StatusOK, http.Json{"status": "ok"})
}

// Sqlite reports the SQLite this side runs and the runtime, for the header.
func (r *BenchSqlController) Sqlite(ctx http.Context) http.Response {
	rows, err := bench.Rows("pragma compile_options")
	if err != nil {
		return ctx.Response().Json(http.StatusInternalServerError, http.Json{"message": err.Error()})
	}
	options := []string{}
	for _, row := range rows {
		options = append(options, row["compile_options"].(string))
	}
	v, _ := bench.Rows("select sqlite_version() AS v")
	return ctx.Response().Json(http.StatusOK, bench.Obj{{K: "version", V: v[0]["v"]}, {K: "compile_options", V: options},
		{K: "runtime", V: bench.Obj{{K: "go", V: runtime.Version()}, {K: "goravel", V: support.Version}, {K: "gomaxprocs", V: runtime.GOMAXPROCS(0)}, {K: "driver", V: "mattn/go-sqlite3 " + bench.SQLiteVersion()}, {K: "gogc", V: os.Getenv("GOGC")}, {K: "goamd64", V: buildSetting("GOAMD64")}}}})
}

func (r *BenchSqlController) Login(ctx http.Context) http.Response {
	data := ctx.Request().All()
	email, ok1 := data["email"].(string)
	password, ok2 := data["password"].(string)
	if !ok1 || !ok2 {
		return ctx.Response().Json(http.StatusUnprocessableEntity, http.Json{"message": "The email and password fields are required."})
	}
	rows, err := bench.Rows("SELECT id, password FROM users WHERE email = ? LIMIT 1", email)
	if err != nil {
		return ctx.Response().Json(http.StatusInternalServerError, http.Json{"message": err.Error()})
	}
	if len(rows) == 0 || !facades.Hash().Check(password, rows[0]["password"].(string)) {
		return ctx.Response().Json(http.StatusUnauthorized, http.Json{"message": "Invalid credentials."})
	}
	raw := make([]byte, 32)
	_, _ = rand.Read(raw)
	token := hex.EncodeToString(raw)
	if _, err := bench.DB().Exec("INSERT INTO tokens (user_id, token_hash, last_used_at, created_at) VALUES (?, ?, ?, ?)", rows[0]["id"], bench.Sha256(token), bench.NowUnix(), bench.Now()); err != nil {
		return ctx.Response().Json(http.StatusInternalServerError, http.Json{"message": err.Error()})
	}
	return ctx.Response().Json(http.StatusOK, http.Json{"token": token})
}

// Index is GET /api/bench/sql/notes?done=false&per_page=15&page=1.
func (r *BenchSqlController) Index(ctx http.Context) http.Response {
	user := bench.CurrentUser(ctx.Value("benchUser"))
	done := 0
	if ctx.Request().Query("done", "false") == "true" {
		done = 1
	}
	perPage := bench.AsInt(ctx.Request().Query("per_page", "15"), 15)
	page := bench.AsInt(ctx.Request().Query("page", "1"), 1)
	total, err := bench.Rows("SELECT count(*) AS total FROM notes WHERE user_id = ? AND done = ? AND deleted_at IS NULL", user.ID, done)
	if err != nil {
		return ctx.Response().Json(http.StatusInternalServerError, http.Json{"message": err.Error()})
	}
	rows, err := bench.Rows("SELECT id, title, body, done, priority, created_at, updated_at FROM notes WHERE user_id = ? AND done = ? AND deleted_at IS NULL ORDER BY priority DESC, id ASC LIMIT ? OFFSET ?", user.ID, done, perPage, (page-1)*perPage)
	if err != nil {
		return ctx.Response().Json(http.StatusInternalServerError, http.Json{"message": err.Error()})
	}
	data := make([]bench.Note, 0, len(rows))
	for _, row := range rows {
		data = append(data, bench.NoteOf(row))
	}
	return ctx.Response().Json(http.StatusOK, bench.Obj{{K: "data", V: data}, {K: "total", V: total[0]["total"]}, {K: "per_page", V: perPage}, {K: "current_page", V: page}})
}

func (r *BenchSqlController) Show(ctx http.Context) http.Response {
	user := bench.CurrentUser(ctx.Value("benchUser"))
	rows, err := bench.Rows("SELECT id, title, body, done, priority, created_at, updated_at FROM notes WHERE id = ? AND user_id = ? AND deleted_at IS NULL LIMIT 1", bench.AsInt(ctx.Request().Route("id"), 0), user.ID)
	if err != nil {
		return ctx.Response().Json(http.StatusInternalServerError, http.Json{"message": err.Error()})
	}
	if len(rows) == 0 {
		return ctx.Response().Json(http.StatusNotFound, http.Json{"message": "Not found."})
	}
	return ctx.Response().Json(http.StatusOK, http.Json{"data": bench.NoteOf(rows[0])})
}

func (r *BenchSqlController) Store(ctx http.Context) http.Response {
	user := bench.CurrentUser(ctx.Value("benchUser"))
	data := ctx.Request().All()
	title, ok := data["title"].(string)
	if !ok || title == "" {
		return ctx.Response().Json(http.StatusUnprocessableEntity, http.Json{"message": "The title field is required."})
	}
	var body any
	if s, ok := data["body"].(string); ok {
		body = s
	}
	priority := int64(3)
	if f, ok := data["priority"].(float64); ok && f == float64(int64(f)) {
		priority = int64(f)
	}
	now := bench.Now()
	rows, err := bench.Rows("INSERT INTO notes (user_id, title, body, done, priority, created_at, updated_at) VALUES (?, ?, ?, 0, ?, ?, ?) RETURNING id", user.ID, title, body, priority, now, now)
	if err != nil {
		return ctx.Response().Json(http.StatusInternalServerError, http.Json{"message": err.Error()})
	}
	return ctx.Response().Json(http.StatusCreated, http.Json{"data": bench.Note{ID: bench.Int64(rows[0]["id"]), Title: title, Body: body, Done: false, Priority: priority, CreatedAt: now, UpdatedAt: now}})
}
