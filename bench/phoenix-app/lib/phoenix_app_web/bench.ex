defmodule PhoenixAppWeb.Bench do
  @moduledoc """
  Measurement tool (BENCH3): raw SQL for the benchmark controllers through
  the repo's connection pool (Ecto.Adapters.SQL.query, no schemas) and the
  helpers shared by them. Answers are ordered objects (small Elixir maps
  are ordered by key, the other sides keep the order of the fields).
  """

  alias Jason.OrderedObject, as: O

  def rows(sql, params \\ []) do
    case Ecto.Adapters.SQL.query!(PhoenixApp.Repo, sql, params) do
      # statements without a result (INSERT, UPDATE) have no columns or rows
      %{columns: cols, rows: rows} when is_list(cols) and is_list(rows) -> Enum.map(rows, fn r -> cols |> Enum.zip(r) |> Map.new() end)
      _ -> []
    end
  end

  def obj(pairs), do: O.new(pairs)

  def reply(conn, status \\ 200, data) do
    conn |> Plug.Conn.put_resp_content_type("application/json") |> Plug.Conn.send_resp(status, Jason.encode_to_iodata!(data))
  end

  def now_text, do: DateTime.utc_now() |> Calendar.strftime("%Y-%m-%d %H:%M:%S")

  # PHP's (int) cast of a query value: leading digits, else 0
  def as_int(nil, default), do: default
  def as_int(value, _default) do
    case Regex.run(~r/^[+-]?\d+/, String.trim(to_string(value))) do
      [d] -> String.to_integer(d)
      _ -> 0
    end
  end

  def sha256(s), do: :crypto.hash(:sha256, s) |> Base.encode16(case: :lower)

  def note(r) do
    obj([{"id", r["id"]}, {"title", r["title"]}, {"body", r["body"]}, {"done", r["done"] == 1}, {"priority", r["priority"]}, {"created_at", r["created_at"]}, {"updated_at", r["updated_at"]}])
  end
end

defmodule PhoenixAppWeb.BenchToken do
  @moduledoc """
  Measurement tool (BENCH3): the token check of the framework-core
  benchmark, the same statements as in the Laravel app; the token's use is
  written at most once a second, as Sanctum does in effect.
  """
  import PhoenixAppWeb.Bench

  def init(opts), do: opts

  def call(conn, _opts) do
    token =
      case Plug.Conn.get_req_header(conn, "authorization") do
        ["Bearer " <> t | _] -> t
        _ -> ""
      end

    found = if token == "", do: [], else: rows("SELECT t.id AS token_id, t.last_used_at, u.id, u.name, u.email FROM tokens t JOIN users u ON u.id = t.user_id WHERE t.token_hash = ? LIMIT 1", [sha256(token)])

    case found do
      [row | _] ->
        now = System.os_time(:second)
        if now - row["last_used_at"] >= 1, do: rows("UPDATE tokens SET last_used_at = ? WHERE id = ?", [now, row["token_id"]])
        Plug.Conn.assign(conn, :bench_user, %{id: row["id"], name: row["name"], email: row["email"]})

      [] ->
        conn |> reply(401, obj([{"message", "Unauthenticated."}])) |> Plug.Conn.halt()
    end
  end
end
