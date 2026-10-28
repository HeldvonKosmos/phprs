// Package bench holds the benchmark's database access and the shared
// helpers of the Goravel comparison app (BENCH3).
package bench

import (
	"crypto/sha256"
	"database/sql"
	"encoding/hex"
	"os"
	"runtime"
	"strconv"
	"strings"
	"sync"
	"time"

	// built with -tags libsqlite3: linked against the benchmark's SQLite
	sqlite3 "github.com/mattn/go-sqlite3"
)

var (
	once sync.Once
	db   *sql.DB
)

// DB is the benchmark's database: raw SQL through database/sql (Goravel's
// ORM is left out, as on every side), the mattn/go-sqlite3 driver linked
// against the benchmark's SQLite; every pooled connection with WAL,
// synchronous FULL, busy_timeout 5000; as many connections as cores.
func DB() *sql.DB {
	once.Do(func() {
		path := os.Getenv("BENCH_SQLITE_DATABASE")
		if path == "" {
			path = "bench.sqlite"
		}
		var err error
		db, err = sql.Open("sqlite3", "file:"+path+"?_journal_mode=WAL&_synchronous=FULL&_busy_timeout=5000")
		if err != nil {
			panic(err)
		}
		n := runtime.GOMAXPROCS(0)
		db.SetMaxOpenConns(n)
		db.SetMaxIdleConns(n)
		db.SetConnMaxLifetime(0)
	})
	return db
}

// Rows runs a query and returns its rows as maps of column to value.
func Rows(query string, args ...any) ([]map[string]any, error) {
	rows, err := DB().Query(query, args...)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	cols, _ := rows.Columns()
	var out []map[string]any
	for rows.Next() {
		vals := make([]any, len(cols))
		ptrs := make([]any, len(cols))
		for i := range vals {
			ptrs[i] = &vals[i]
		}
		if err := rows.Scan(ptrs...); err != nil {
			return nil, err
		}
		m := make(map[string]any, len(cols))
		for i, c := range cols {
			if b, ok := vals[i].([]byte); ok {
				m[c] = string(b)
			} else {
				m[c] = vals[i]
			}
		}
		out = append(out, m)
	}
	return out, rows.Err()
}

// SQLiteVersion is the library's version as the driver reports it.
func SQLiteVersion() string {
	v, _, _ := sqlite3.Version()
	return v
}

func Sha256(s string) string {
	h := sha256.Sum256([]byte(s))
	return hex.EncodeToString(h[:])
}

func Now() string {
	return time.Now().UTC().Format("2006-01-02 15:04:05")
}

// AsInt is PHP's (int) cast of a query value: leading digits, else 0.
func AsInt(value string, dflt int) int {
	s := strings.TrimSpace(value)
	if value == "" {
		return dflt
	}
	end := 0
	for i, c := range s {
		if (c >= '0' && c <= '9') || (i == 0 && (c == '-' || c == '+')) {
			end = i + 1
			continue
		}
		break
	}
	n, err := strconv.Atoi(s[:end])
	if err != nil {
		return 0
	}
	return n
}

// Int64 of a database value (SQLite integers come as int64).
func Int64(v any) int64 {
	switch x := v.(type) {
	case int64:
		return x
	case float64:
		return int64(x)
	}
	return 0
}

// Str is a nullable text column: nil stays nil.
func Str(v any) any {
	if v == nil {
		return nil
	}
	if s, ok := v.(string); ok {
		return s
	}
	return v
}

func NowUnix() int64 {
	return time.Now().Unix()
}

// User is the authenticated user of a request (BenchToken).
type User struct {
	ID    int64
	Name  any
	Email any
}

// CurrentUser is the user BenchToken put into the context.
func CurrentUser(value any) User {
	u, _ := value.(User)
	return u
}
