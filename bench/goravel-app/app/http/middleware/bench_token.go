package middleware

import (
	"github.com/goravel/framework/contracts/http"

	"goravel/app/bench"
)

// BenchToken is the token check of the framework-core benchmark, the same
// statements as in the Laravel app; the token's use is written at most once
// a second, as Sanctum does in effect.
type BenchToken struct{}

func (m *BenchToken) Signature() string {
	return "bench_token"
}

func (m *BenchToken) Handle(ctx http.Context) {
	{
		header := ctx.Request().Header("Authorization", "")
		token := ""
		if len(header) > 7 && header[:7] == "Bearer " {
			token = header[7:]
		}
		if token == "" {
			_ = ctx.Response().Json(http.StatusUnauthorized, http.Json{"message": "Unauthenticated."}).Abort()
			return
		}
		rows, err := bench.Rows("SELECT t.id AS token_id, t.last_used_at, u.id, u.name, u.email FROM tokens t JOIN users u ON u.id = t.user_id WHERE t.token_hash = ? LIMIT 1", bench.Sha256(token))
		if err != nil || len(rows) == 0 {
			_ = ctx.Response().Json(http.StatusUnauthorized, http.Json{"message": "Unauthenticated."}).Abort()
			return
		}
		row := rows[0]
		now := bench.NowUnix()
		if now-bench.Int64(row["last_used_at"]) >= 1 {
			if _, err := bench.DB().Exec("UPDATE tokens SET last_used_at = ? WHERE id = ?", now, row["token_id"]); err != nil {
				_ = ctx.Response().Json(http.StatusInternalServerError, http.Json{"message": err.Error()}).Abort()
				return
			}
		}
		ctx.WithValue("benchUser", bench.User{ID: bench.Int64(row["id"]), Name: row["name"], Email: row["email"]})
		ctx.Request().Next()
	}
}
