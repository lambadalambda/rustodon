# frozen_string_literal: true

# Fresh local identity only: no remote actors, follows, or statuses are seeded.
['', '_recipient', '_outsider'].each do |suffix|
  username = "#{ENV.fetch('PEER_USERNAME')}#{suffix}"
  account = Account.create!(username: username, display_name: 'Disposable peer')
  user = User.new(email: "#{account.username}@#{ENV.fetch('LOCAL_DOMAIN')}", password: 'Peer-only-password-123!', account: account, approved: true, agreement: true)
  user.skip_confirmation!
  user.save!
  # The create callback derives approval from registrations_mode, not the constructor.
  user.approve!
  raise 'local seed user is not functional' unless user.reload.functional?
  application = Doorkeeper::Application.create!(name: 'Peer smoke', redirect_uri: 'urn:ietf:wg:oauth:2.0:oob', scopes: 'read write follow')
  Doorkeeper::AccessToken.create!(application: application, resource_owner_id: user.id, scopes: 'read write follow')
end
Account.representative
