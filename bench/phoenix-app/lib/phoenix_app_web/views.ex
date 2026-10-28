defmodule PhoenixAppWeb.UserView do
  @moduledoc "Measurement tool (BENCH3, THESIS1): the user of a hydrated note."
  defstruct [:id, :name, :email]

  def to_object(%__MODULE__{} = u), do: PhoenixAppWeb.Bench.obj([{"id", u.id}, {"name", u.name}, {"email", u.email}])
end

defmodule PhoenixAppWeb.NoteView do
  @moduledoc "Measurement tool (BENCH3, THESIS1): a note hydrated from a raw row, with its user."
  alias PhoenixAppWeb.UserView
  defstruct [:id, :title, :body, :done, :priority, :created_at, :user]

  def from_row(r) do
    %__MODULE__{id: r["id"], title: r["title"], body: r["body"], done: r["done"] == 1, priority: r["priority"], created_at: r["created_at"], user: %UserView{id: r["user_id"], name: r["user_name"], email: r["user_email"]}}
  end

  def to_object(%__MODULE__{} = n) do
    PhoenixAppWeb.Bench.obj([{"id", n.id}, {"title", n.title}, {"body", n.body}, {"done", n.done}, {"priority", n.priority}, {"created_at", n.created_at}, {"user", UserView.to_object(n.user)}])
  end
end
