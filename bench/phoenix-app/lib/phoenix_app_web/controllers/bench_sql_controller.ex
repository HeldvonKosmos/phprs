defmodule PhoenixAppWeb.BenchSqlController do
  @moduledoc """
  Measurement tool (BENCH3): the framework-core benchmark with raw SQL on
  SQLite; the same statements and answers as the Laravel app's
  BenchSqlController.
  """
  use PhoenixAppWeb, :controller
  import Phoenix.Controller, except: [json: 2]
  import PhoenixAppWeb.Bench

  def health(conn, _params), do: reply(conn, obj([{"status", "ok"}]))

  # the SQLite this side runs and the runtime, for the header
  def sqlite(conn, _params) do
    [%{"v" => version}] = rows("select sqlite_version() AS v")
    options = rows("pragma compile_options") |> Enum.map(& &1["compile_options"])
    runtime = obj([{"elixir", System.version()}, {"otp", to_string(:erlang.system_info(:otp_release))}, {"schedulers", :erlang.system_info(:schedulers_online)}, {"jit", :erlang.system_info(:emu_flavor) == :jit}, {"phoenix", to_string(Application.spec(:phoenix, :vsn))}])
    reply(conn, obj([{"version", version}, {"compile_options", options}, {"runtime", runtime}]))
  end

  def login(conn, _params) do
    p = conn.body_params
    email = p["email"]
    password = p["password"]

    if not (is_binary(email) and is_binary(password)) do
      reply(conn, 422, obj([{"message", "The email and password fields are required."}]))
    else
      case rows("SELECT id, password FROM users WHERE email = ? LIMIT 1", [email]) do
        [row | _] ->
          if Bcrypt.verify_pass(password, row["password"]) do
            token = :crypto.strong_rand_bytes(32) |> Base.encode16(case: :lower)
            rows("INSERT INTO tokens (user_id, token_hash, last_used_at, created_at) VALUES (?, ?, ?, ?)", [row["id"], sha256(token), System.os_time(:second), now_text()])
            reply(conn, obj([{"token", token}]))
          else
            reply(conn, 401, obj([{"message", "Invalid credentials."}]))
          end

        [] ->
          reply(conn, 401, obj([{"message", "Invalid credentials."}]))
      end
    end
  end

  # GET /api/bench/sql/notes?done=false&per_page=15&page=1
  def index(conn, _params) do
    q = conn.query_params
    user = conn.assigns.bench_user
    done = if Map.get(q, "done", "false") == "true", do: 1, else: 0
    per_page = as_int(q["per_page"], 15)
    page = as_int(q["page"], 1)
    [%{"total" => total}] = rows("SELECT count(*) AS total FROM notes WHERE user_id = ? AND done = ? AND deleted_at IS NULL", [user.id, done])
    found = rows("SELECT id, title, body, done, priority, created_at, updated_at FROM notes WHERE user_id = ? AND done = ? AND deleted_at IS NULL ORDER BY priority DESC, id ASC LIMIT ? OFFSET ?", [user.id, done, per_page, (page - 1) * per_page])
    reply(conn, obj([{"data", Enum.map(found, &note/1)}, {"total", total}, {"per_page", per_page}, {"current_page", page}]))
  end

  def show(conn, %{"id" => id}) do
    case rows("SELECT id, title, body, done, priority, created_at, updated_at FROM notes WHERE id = ? AND user_id = ? AND deleted_at IS NULL LIMIT 1", [as_int(id, 0), conn.assigns.bench_user.id]) do
      [row | _] -> reply(conn, obj([{"data", note(row)}]))
      [] -> reply(conn, 404, obj([{"message", "Not found."}]))
    end
  end

  def store(conn, _params) do
    p = conn.body_params
    title = p["title"]

    if not (is_binary(title) and title != "") do
      reply(conn, 422, obj([{"message", "The title field is required."}]))
    else
      text = if is_binary(p["body"]), do: p["body"], else: nil
      priority = Map.get(p, "priority", 3)
      priority = if is_integer(priority), do: priority, else: 3
      t = now_text()
      # exqlite returns the RETURNING row without column names: by position
      %{rows: [[id]]} = Ecto.Adapters.SQL.query!(PhoenixApp.Repo, "INSERT INTO notes (user_id, title, body, done, priority, created_at, updated_at) VALUES (?, ?, ?, 0, ?, ?, ?) RETURNING id", [conn.assigns.bench_user.id, title, text, priority, t, t])
      reply(conn, 201, obj([{"data", obj([{"id", id}, {"title", title}, {"body", text}, {"done", false}, {"priority", priority}, {"created_at", t}, {"updated_at", t}])}]))
    end
  end
end
