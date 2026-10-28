# Measurement tool (BENCH3, THESIS1): workloads with an adjustable amount of
# application logic per request (`n`), the same steps and answers as the
# Laravel app's BenchLogicController.
class BenchLogicController < ApplicationController
  before_action :bench_token

  # n notes with their user, hydrated into objects and listed
  def hydrate
    render json: { data: rows(n).map { |r| Bench::NoteView.from_row(r).to_h } }
  end

  # n notes prepared with Enumerable: map, filter, group_by, sort_by, sum
  def collect
    mapped = rows(n).map { |r| { title: r["title"], priority: r["priority"], done: r["done"] == 1, chars: r["title"].length + r["body"].to_s.length } }
    groups = mapped.reject { |r| r[:done] }.group_by { |r| r[:priority] }
    out = groups.keys.sort.to_h do |k|
      g = groups[k]
      [k.to_s, { count: g.size, chars: g.sum { |r| r[:chars] }, first: g.sort_by { |r| r[:title] }.first[:title] }]
    end
    render json: { groups: out }
  end

  ORDER = %w[items title priority tags tag].freeze

  # a nested input of n items, validated with Active Model; answers the
  # number of items (errors in Laravel's form)
  def validate
    items = body["items"]
    found = []
    if items.nil? || (items.is_a?(Array) && items.empty?)
      found << [0, [], "items", "The items field is required."]
    elsif !items.is_a?(Array)
      found << [0, [], "items", "The items field must be an array."]
    else
      items.each_with_index do |data, i|
        item = Bench::Item.new(data)
        next if item.valid?
        item.errors.each do |e|
          attr = e.attribute.to_s
          key = "items.#{i}.#{attr}"
          rank = attr.start_with?("tags.") ? "tag" : attr
          message = case e.type
                    when :blank then "The #{key} field is required."
                    when :not_a_string then "The #{key} field must be a string."
                    when :too_long then "The #{key} field must not be greater than #{e.options[:count]} characters."
                    when :not_a_number, :not_an_integer then "The #{key} field must be an integer."
                    when :in, :greater_than_or_equal_to, :less_than_or_equal_to then "The #{key} field must be between 1 and 5."
                    when :not_an_array then "The #{key} field must be an array."
                    else "The #{key} field is invalid."
                    end
          found << [ORDER.index(rank), [i] + attr.scan(/\d+/).map(&:to_i), key, message]
        end
      end
    end
    return render(json: { valid: items.size }) if found.empty?
    errors = {}
    found.sort_by { |r| [r[0], r[1]] }.each { |_, _, key, msg| (errors[key] ||= []) << msg }
    more = errors.values.flatten.size - 1
    first = errors.values.first.first
    render json: { message: first + (more.positive? ? " (and #{more} more #{more == 1 ? 'error' : 'errors'})" : ""), errors: errors }, status: 422
  end

  # a nested structure of depth 3 with n items, no database
  def json
    count = n
    items = (0...count).map { |i| { id: i, name: "item #{i}", tags: %w[a b c], meta: { created: "2026-10-27", score: i * 1.5, flags: { x: true, y: false } } } }
    render json: { count: count, items: items }
  end

  private

  def n
    [[as_int(request.query_parameters["n"], 1), 0].max, 1000].min
  end

  def rows(count)
    sql("SELECT n.id, n.title, n.body, n.done, n.priority, n.created_at, u.id AS user_id, u.name AS user_name, u.email AS user_email FROM notes n JOIN users u ON u.id = n.user_id WHERE n.user_id = ? AND n.deleted_at IS NULL ORDER BY n.id LIMIT ?", [@bench_user[:id], count])
  end
end
