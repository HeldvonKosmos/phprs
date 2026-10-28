# Routes of the Rails comparison app (BENCH3), as in the Laravel app.
Rails.application.routes.draw do
  get "health", to: "bench_sql#health"
  scope "api/bench/sql" do
    get "health", to: "bench_sql#health"
    get "sqlite", to: "bench_sql#sqlite"
    post "login", to: "bench_sql#login"
    get "notes", to: "bench_sql#index"
    post "notes", to: "bench_sql#store"
    get "notes/:id", to: "bench_sql#show"
  end
  scope "api/bench/logic" do
    get "hydrate", to: "bench_logic#hydrate"
    get "collect", to: "bench_logic#collect"
    post "validate", to: "bench_logic#validate"
    get "json", to: "bench_logic#json"
  end
end
