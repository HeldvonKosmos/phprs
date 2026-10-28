module Bench
  # Measurement tool (BENCH3): one element of the validate workload, with
  # Active Model validations; the rules of the Laravel app.
  class Item
    include ActiveModel::Validations

    attr_reader :title, :priority, :tags

    def initialize(data)
      data = {} unless data.is_a?(Hash)
      @title, @priority, @tags, @given_tags = data["title"], data["priority"], data["tags"], data.key?("tags")
    end

    validates :title, presence: true
    validate :title_is_string
    validates :title, length: { maximum: 120 }, if: -> { title.is_a?(String) }
    validates :priority, presence: true
    validates :priority, numericality: { only_integer: true, in: 1..5 }, unless: -> { priority.nil? }
    validate :tags_are_strings

    private

    def title_is_string
      errors.add(:title, :not_a_string) if !title.nil? && !title.is_a?(String)
    end

    def tags_are_strings
      return if tags.nil?
      return errors.add(:tags, :not_an_array) unless tags.is_a?(Array)
      tags.each_with_index do |t, j|
        if !t.is_a?(String)
          errors.add(:"tags.#{j}", :not_a_string)
        elsif t.length > 20
          errors.add(:"tags.#{j}", :too_long, count: 20)
        end
      end
    end
  end
end
