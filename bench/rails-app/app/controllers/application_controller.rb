# Measurement tool (BENCH3): the base of the benchmark controllers, raw SQL
# through Active Record's connection (no models) and the token check of the
# framework-core benchmark (BenchToken).
class ApplicationController < ActionController::API
  private

  def sql(query, binds = [])
    ActiveRecord::Base.with_connection { |c| c.select_all(query, "SQL", binds).to_a }
  end

  def body
    @body ||= begin
      JSON.parse(request.raw_post.presence || "{}")
    rescue JSON::ParserError
      {}
    end
  end

  def now_text
    Time.now.utc.strftime("%Y-%m-%d %H:%M:%S")
  end

  # PHP's (int) cast of a query value: leading digits, else 0
  def as_int(value, default)
    (value.nil? ? default.to_s : value.to_s).strip[/\A[+-]?\d+/].to_i
  end

  # the token's use is written at most once a second, as Sanctum does in effect
  def bench_token
    header = request.headers["Authorization"].to_s
    token = header.start_with?("Bearer ") ? header[7..] : ""
    return render(json: { message: "Unauthenticated." }, status: 401) if token.empty?
    row = sql("SELECT t.id AS token_id, t.last_used_at, u.id, u.name, u.email FROM tokens t JOIN users u ON u.id = t.user_id WHERE t.token_hash = ? LIMIT 1", [Digest::SHA256.hexdigest(token)]).first
    return render(json: { message: "Unauthenticated." }, status: 401) if row.nil?
    now = Time.now.to_i
    sql("UPDATE tokens SET last_used_at = ? WHERE id = ?", [now, row["token_id"]]) if now - row["last_used_at"] >= 1
    @bench_user = { id: row["id"], name: row["name"], email: row["email"] }
  end
end
