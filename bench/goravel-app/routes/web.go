package routes

import (
	"github.com/goravel/framework/contracts/route"

	"goravel/app/facades"
	"goravel/app/http/controllers"
	"goravel/app/http/middleware"
)

// Web holds the benchmark's routes (BENCH3): the same paths as the
// Laravel app's routes/api.php and OpenGrid's benchsql module.
func Web() {
	sql := controllers.NewBenchSqlController()
	logic := controllers.NewBenchLogicController()
	facades.Route().Get("/health", sql.Health)
	facades.Route().Prefix("/api/bench/sql").Group(func(router route.Router) {
		router.Get("/health", sql.Health)
		router.Get("/sqlite", sql.Sqlite)
		router.Post("/login", sql.Login)
	})
	facades.Route().Prefix("/api/bench/sql/notes").Middleware(&middleware.BenchToken{}).Group(func(router route.Router) {
		router.Get("", sql.Index)
		router.Post("", sql.Store)
		router.Get("/{id}", sql.Show)
	})
	facades.Route().Prefix("/api/bench/logic").Middleware(&middleware.BenchToken{}).Group(func(router route.Router) {
		router.Get("/hydrate", logic.Hydrate)
		router.Get("/collect", logic.Collect)
		router.Post("/validate", logic.Validate)
		router.Get("/json", logic.Json)
	})
}
