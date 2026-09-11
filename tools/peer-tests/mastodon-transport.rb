# frozen_string_literal: true

# Mounted only into disposable pinned-image containers, never upstream source.
# Keep HTTP.rb's TLS/SNI, signatures, redirects and response bounds intact.
Rails.application.config.after_initialize do
  require 'json'
  require 'socket'
  origins = JSON.parse(ENV.fetch('RUSTODON_TEST_PEER_ORIGINS'))
  raise 'unexpected peer map' unless origins.keys.sort == %w[https://mastodon.peer.invalid https://rustodon.peer.invalid]

  endpoints = origins.to_h do |origin, endpoint|
    host, port = endpoint.split(':')
    raise 'non-loopback peer endpoint' unless host == '127.0.0.1' && port.match?(/\A[0-9]+\z/) && (1024..65535).cover?(port.to_i)

    [URI(origin).host, [host, port.to_i]]
  end.freeze
  transport = Module.new do
    define_method(:open) do |host, port, *_args|
      raise SocketError, 'unmapped peer destination' unless port.to_i == 443 && endpoints.key?(host)

      TCPSocket.new(*endpoints.fetch(host))
    end
    alias_method :new, :open
  end
  Request::Socket.singleton_class.prepend(transport)
end
