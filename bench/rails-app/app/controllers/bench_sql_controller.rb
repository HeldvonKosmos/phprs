# Measurement tool (BENCH3): the framework-core benchmark with raw SQL on
# SQLite; the same statements and answers as the Laravel app's
# BenchSqlController.
class BenchSqlController < ApplicationController
  before_action :bench_token, only: %i[index show store]

  def health
    render json: { status: "ok" }
  end

  # the SQLite this side runs and the runtime, for the header
  def sqlite
    render json: { version: sql("select sqlite_version() AS v").first["v"], compile_options: sql("pragma compile_options").map { |r| r["compile_options"] },
                   runtime: { ruby: RUBY_VERSION, yjit: defined?(RubyVM::YJIT) && RubyVM::YJIT.enabled?, rails: Rails.version, puma_threads: ENV.fetch("RAILS_MAX_THREADS", 3).to_i,
                              jemalloc: File.read("/proc/self/maps").include?("jemalloc") } }
  end

  def login
    email, password = body["email"], body["password"]
    return render(json: { message: "The email and password fields are required." }, status: 422) unless email.is_a?(String) && password.is_a?(String)
    row = sql("SELECT id, password FROM users WHERE email = ? LIMIT 1", [email]).first
    return render(json: { message: "Invalid credentials." }, status: 401) if row.nil? || BCrypt::Password.new(row["password"]) != password
    token = SecureRandom.hex(32)
    sql("INSERT INTO tokens (user_id, token_hash, last_used_at, created_at) VALUES (?, ?, ?, ?)", [row["id"], Digest::SHA256.hexdigest(token), Time.now.to_i, now_text])
    render json: { token: token }
  end

  # GET /api/bench/sql/notes?done=false&per_page=15&page=1
  def index
    done = request.query_parameters.fetch("done", "false") == "true" ? 1 : 0
    per_page = as_int(request.query_parameters["per_page"], 15)
    page = as_int(request.query_parameters["page"], 1)
    total = sql("SELECT count(*) AS total FROM notes WHERE user_id = ? AND done = ? AND deleted_at IS NULL", [@bench_user[:id], done]).first["total"]
    rows = sql("SELECT id, title, body, done, priority, created_at, updated_at FROM notes WHERE user_id = ? AND done = ? AND deleted_at IS NULL ORDER BY priority DESC, id ASC LIMIT ? OFFSET ?", [@bench_user[:id], done, per_page, (page - 1) * per_page])
    render json: { data: rows.map { |r| note(r) }, total: total, per_page: per_page, current_page: page }
  end

  def show
    row = sql("SELECT id, title, body, done, priority, created_at, updated_at FROM notes WHERE id = ? AND user_id = ? AND deleted_at IS NULL LIMIT 1", [as_int(params[:id], 0), @bench_user[:id]]).first
    return render(json: { message: "Not found." }, status: 404) if row.nil?
    render json: { data: note(row) }
  end

  def store
    title = body["title"]
    return render(json: { message: "The title field is required." }, status: 422) unless title.is_a?(String) && !title.empty?
    text = body["body"].is_a?(String) ? body["body"] : nil
    priority = body.fetch("priority", 3)
    priority = 3 unless priority.is_a?(Integer)
    now = now_text
    id = sql("INSERT INTO notes (user_id, title, body, done, priority, created_at, updated_at) VALUES (?, ?, ?, 0, ?, ?, ?) RETURNING id", [@bench_user[:id], title, text, priority, now, now]).first["id"]
    render json: { data: { id: id, title: title, body: text, done: false, priority: priority, created_at: now, updated_at: now } }, status: 201
  end

  private

  def note(r)
    { id: r["id"], title: r["title"], body: r["body"], done: r["done"] == 1, priority: r["priority"], created_at: r["created_at"], updated_at: r["updated_at"] }
  end
end
