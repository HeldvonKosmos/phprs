module Bench
  # Measurement tool (BENCH3, THESIS1): a note hydrated from a raw row, with its user.
  class NoteView
    def initialize(id, title, body, done, priority, created_at, user)
      @id, @title, @body, @done, @priority, @created_at, @user = id, title, body, done, priority, created_at, user
    end

    def self.from_row(r)
      new(r["id"], r["title"], r["body"], r["done"] == 1, r["priority"], r["created_at"], UserView.new(r["user_id"], r["user_name"], r["user_email"]))
    end

    def to_h
      { id: @id, title: @title, body: @body, done: @done, priority: @priority, created_at: @created_at, user: @user.to_h }
    end
  end
end
