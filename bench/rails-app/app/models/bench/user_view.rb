module Bench
  # Measurement tool (BENCH3, THESIS1): the user of a hydrated note.
  class UserView
    def initialize(id, name, email)
      @id, @name, @email = id, name, email
    end

    def to_h
      { id: @id, name: @name, email: @email }
    end
  end
end
