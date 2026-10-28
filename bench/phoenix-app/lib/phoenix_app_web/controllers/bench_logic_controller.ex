defmodule PhoenixAppWeb.BenchLogicController do
  @moduledoc """
  Measurement tool (BENCH3, THESIS1): workloads with an adjustable amount of
  application logic per request (`n`), the same steps and answers as the
  Laravel app's BenchLogicController.
  """
  use PhoenixAppWeb, :controller
  import Phoenix.Controller, except: [json: 2]
  import PhoenixAppWeb.Bench
  alias PhoenixAppWeb.NoteView

  defp n(conn), do: conn.query_params["n"] |> as_int(1) |> max(0) |> min(1000)

  defp logic_rows(conn, n) do
    rows("SELECT n.id, n.title, n.body, n.done, n.priority, n.created_at, u.id AS user_id, u.name AS user_name, u.email AS user_email FROM notes n JOIN users u ON u.id = n.user_id WHERE n.user_id = ? AND n.deleted_at IS NULL ORDER BY n.id LIMIT ?", [conn.assigns.bench_user.id, n])
  end

  # n notes with their user, hydrated into structs and listed
  def hydrate(conn, _params) do
    reply(conn, obj([{"data", conn |> logic_rows(n(conn)) |> Enum.map(&(&1 |> NoteView.from_row() |> NoteView.to_object()))}]))
  end

  # n notes prepared with Enum: map, filter, group_by, sort_by, sum
  def collect(conn, _params) do
    groups =
      conn
      |> logic_rows(n(conn))
      |> Enum.map(fn r -> %{title: r["title"], priority: r["priority"], done: r["done"] == 1, chars: String.length(r["title"]) + String.length(r["body"] || "")} end)
      |> Enum.reject(& &1.done)
      |> Enum.group_by(& &1.priority)
      |> Enum.sort_by(fn {k, _} -> k end)
      |> Enum.map(fn {k, g} -> {Integer.to_string(k), obj([{"count", length(g)}, {"chars", g |> Enum.map(& &1.chars) |> Enum.sum()}, {"first", (g |> Enum.sort_by(& &1.title) |> hd()).title}])} end)

    reply(conn, obj([{"groups", obj(groups)}]))
  end

  @rank %{"items" => 0, "title" => 1, "priority" => 2, "tags" => 3, "tag" => 4}

  # one element: a schemaless Ecto.Changeset (cast checks the types, then the
  # constraints); the list of tags is checked per element, as Laravel names
  # the element (items.5.tags.1)
  defp item_errors(data, i) do
    p = "items.#{i}"
    data = if is_map(data), do: data, else: %{}
    cs =
      {%{}, %{title: :string, priority: :integer}}
      |> Ecto.Changeset.cast(data, [:title, :priority])
      |> Ecto.Changeset.validate_required([:title, :priority])
      |> Ecto.Changeset.validate_length(:title, max: 120)
      |> Ecto.Changeset.validate_number(:priority, greater_than_or_equal_to: 1, less_than_or_equal_to: 5)

    field_errors =
      cs.errors
      |> Enum.uniq_by(fn {f, _} -> f end)
      |> Enum.map(fn {field, {_msg, opts}} ->
        key = "#{p}.#{field}"
        message =
          case {field, opts[:validation]} do
            {_, :required} -> "The #{key} field is required."
            {:title, :cast} -> "The #{key} field must be a string."
            {:title, :length} -> "The #{key} field must not be greater than 120 characters."
            {:priority, :cast} -> "The #{key} field must be an integer."
            {:priority, :number} -> "The #{key} field must be between 1 and 5."
            _ -> "The #{key} field is invalid."
          end
        {@rank[Atom.to_string(field)], [i], key, message}
      end)

    tag_errors =
      case Map.get(data, "tags") do
        nil -> []
        tags when is_list(tags) ->
          tags
          |> Enum.with_index()
          |> Enum.flat_map(fn {t, j} ->
            key = "#{p}.tags.#{j}"
            cond do
              not is_binary(t) -> [{4, [i, j], key, "The #{key} field must be a string."}]
              String.length(t) > 20 -> [{4, [i, j], key, "The #{key} field must not be greater than 20 characters."}]
              true -> []
            end
          end)
        _ -> [{3, [i], "#{p}.tags", "The #{p}.tags field must be an array."}]
      end

    field_errors ++ tag_errors
  end

  # a nested input of n items, validated; answers the number of items
  # (errors in Laravel's form)
  def validate(conn, _params) do
    items = conn.body_params["items"]

    found =
      cond do
        is_nil(items) or items == [] -> [{0, [], "items", "The items field is required."}]
        not is_list(items) -> [{0, [], "items", "The items field must be an array."}]
        true -> items |> Enum.with_index() |> Enum.flat_map(fn {d, i} -> item_errors(d, i) end)
      end

    case Enum.sort_by(found, fn {rank, idx, _, _} -> {rank, idx} end) do
      [] ->
        reply(conn, obj([{"valid", length(items)}]))

      sorted ->
        errors = sorted |> Enum.reduce([], fn {_, _, k, m}, acc -> acc ++ [{k, [m]}] end)
        more = length(sorted) - 1
        {_, _, _, first} = hd(sorted)
        suffix = if more > 0, do: " (and #{more} more #{if more == 1, do: "error", else: "errors"})", else: ""
        reply(conn, 422, obj([{"message", first <> suffix}, {"errors", obj(errors)}]))
    end
  end

  # a nested structure of depth 3 with n items, no database
  def json_workload(conn, _params) do
    count = n(conn)
    items = for i <- 0..(count - 1)//1, do: obj([{"id", i}, {"name", "item #{i}"}, {"tags", ["a", "b", "c"]}, {"meta", obj([{"created", "2026-10-27"}, {"score", i * 1.5}, {"flags", obj([{"x", true}, {"y", false}])}])}])
    PhoenixAppWeb.Bench.reply(conn, obj([{"count", count}, {"items", items}]))
  end
end
