package bootstrap

import (
	"github.com/goravel/framework/contracts/foundation"
	"github.com/goravel/framework/hash"
	"github.com/goravel/framework/http"
	"github.com/goravel/framework/log"
	"github.com/goravel/framework/route"
	"github.com/goravel/framework/validation"
	"github.com/goravel/gin"
)

// BENCH3: the providers the benchmark app uses; the skeleton's database
// (ORM and its PostgreSQL driver), queue, mail, gRPC, AI, session, cache
// and view providers are left out. The benchmark's raw SQL goes through
// database/sql (app/bench/db.go).
func Providers() []foundation.ServiceProvider {
	return []foundation.ServiceProvider{
		&log.ServiceProvider{},
		&hash.ServiceProvider{},
		&http.ServiceProvider{},
		&validation.ServiceProvider{},
		&route.ServiceProvider{},
		&gin.ServiceProvider{},
	}
}
