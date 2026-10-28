defmodule PhoenixAppWeb.Router do
  use PhoenixAppWeb, :router

  # Routes of the Phoenix comparison app (BENCH3), as in the Laravel app.
  pipeline :api do
    plug :accepts, ["json"]
  end

  pipeline :bench_token do
    plug PhoenixAppWeb.BenchToken
  end

  scope "/", PhoenixAppWeb do
    pipe_through :api
    get "/health", BenchSqlController, :health
    get "/api/bench/sql/health", BenchSqlController, :health
    get "/api/bench/sql/sqlite", BenchSqlController, :sqlite
    post "/api/bench/sql/login", BenchSqlController, :login
  end

  scope "/api/bench", PhoenixAppWeb do
    pipe_through [:api, :bench_token]
    get "/sql/notes", BenchSqlController, :index
    post "/sql/notes", BenchSqlController, :store
    get "/sql/notes/:id", BenchSqlController, :show
    get "/logic/hydrate", BenchLogicController, :hydrate
    get "/logic/collect", BenchLogicController, :collect
    post "/logic/validate", BenchLogicController, :validate
    get "/logic/json", BenchLogicController, :json_workload
  end
end
