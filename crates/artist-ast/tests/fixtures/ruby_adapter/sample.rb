# Fixture for the Ruby adapter integration tests.
module Billing
  VERSION = "1.0".freeze

  class Account
    attr_accessor :email
    attr_reader :id

    def initialize(email)
      @email = email
    end

    def public_method
      :ok
    end

    private

    def secret
      :hidden
    end

    protected

    def helper
      :assist
    end
  end

  class User < Account
    has_many :posts

    def self.find(id)
      new(id)
    end
  end
end
