# frozen_string_literal: true

require 'openssl'
require 'digest'
require 'fastimage'
require 'json'

ActiveRecord::Migration.check_all_pending!

expected = {
  'mention' => ['Mention', 7001, 116_844_606_259_201_001, 116_844_606_259_202_001],
  'status' => ['Status', 116_845_101_711_365_104, 116_844_606_259_201_001, 116_844_606_259_202_001],
  'reblog' => ['Status', 116_845_321_912_325_301, 116_844_606_259_201_001, 116_844_606_259_202_001],
  'follow' => ['Follow', 8002, 116_844_606_259_201_001, 116_844_606_259_202_001],
  'follow_request' => ['FollowRequest', 8003, 116_844_606_259_201_001, 116_844_606_259_202_002],
  'favourite' => ['Favourite', 8101, 116_844_606_259_201_001, 116_844_606_259_202_001],
  'poll' => ['Poll', 8201, 116_844_606_259_201_001, 116_844_606_259_202_001],
  'update' => ['Status', 116_845_105_643_525_105, 116_844_606_259_201_001, 116_844_606_259_202_001],
  'severed_relationships' => ['AccountRelationshipSeveranceEvent', 8302, 116_844_606_259_201_001, 116_844_606_259_201_001],
  'moderation_warning' => ['AccountWarning', 8401, 116_844_606_259_201_001, 116_844_606_259_201_001],
  'annual_report' => ['GeneratedAnnualReport', 8501, 116_844_606_259_201_001, 116_844_606_259_201_001],
  'admin.sign_up' => ['Account', 116_844_606_259_201_003, 116_844_606_259_201_002, 116_844_606_259_201_003],
  'admin.report' => ['Report', 8601, 116_844_606_259_201_002, 116_844_606_259_201_001],
  'quote' => ['Quote', 116_845_317_980_168_701, 116_844_606_259_201_001, 116_844_606_259_202_001],
  'quoted_update' => ['Status', 116_845_314_048_005_201, 116_844_606_259_201_001, 116_844_606_259_202_001],
  'added_to_collection' => ['CollectionItem', 116_845_549_977_608_802, 116_844_606_259_201_001, 116_844_606_259_202_001],
  'collection_update' => ['Collection', 116_845_549_977_608_801, 116_844_606_259_201_001, 116_844_606_259_202_001],
}.freeze

notifications = Notification.order(:id).to_a
raise "expected 17 notifications, found #{notifications.size}" unless notifications.size == expected.size
groupable_types = %w(favourite reblog follow admin.sign_up).freeze

notifications.each do |notification|
  activity_class, activity_id, account_id, from_account_id = expected.fetch(notification.type.to_s)
  activity = notification.activity
  raise "#{notification.type} activity is unreadable" if activity.nil?
  raise "#{notification.type} activity class mismatch" unless activity.class.name == activity_class
  raise "#{notification.type} activity ID mismatch" unless activity.id == activity_id
  raise "#{notification.type} recipient mismatch" unless notification.account_id == account_id
  raise "#{notification.type} from-account mismatch" unless notification.from_account.id == from_account_id

  recipient_user = notification.account.user
  raise "#{notification.type} recipient has no local user" if recipient_user.nil?

  if groupable_types.include?(notification.type.to_s)
    prefix = if %i(favourite reblog).include?(notification.type)
               "#{notification.type}-#{notification.target_status.id}"
             else
               notification.type.to_s
             end
    expected_group_key = "#{prefix}-#{notification.activity.created_at.utc.to_i / 1.hour.to_i}"
    raise "#{notification.type} group key mismatch" unless notification.group_key == expected_group_key
  end

  if notification.type == :'admin.report'
    raise 'admin.report recipient cannot manage reports' unless recipient_user.role.can?(:manage_reports)
  elsif notification.type == :'admin.sign_up'
    raise 'admin.sign_up recipient cannot manage users' unless recipient_user.role.can?(:manage_users)
  end

  ActiveModelSerializers::SerializableResource.new(
    notification,
    serializer: REST::NotificationSerializer,
    scope: recipient_user,
    scope_name: :current_user
  ).as_json
end

snowflake_records = [
  Account.all,
  Status.unscoped.all,
  MediaAttachment.all,
  Quote.all,
  Collection.all,
  CollectionItem.all,
  NotificationRequest.all,
].flat_map(&:to_a)
snowflake_records.each do |record|
  decoded = Mastodon::Snowflake.to_time(record.id)
  raise "#{record.class} #{record.id} Snowflake timestamp mismatch" unless decoded == record.created_at.utc
end

expected_sequences = {
  'accounts_id_seq' => [5, true],
  'statuses_id_seq' => [13, true],
  'media_attachments_id_seq' => [1, true],
  'quotes_id_seq' => [2, true],
  'collections_id_seq' => [1, true],
  'collection_items_id_seq' => [1, true],
  'notification_requests_id_seq' => [1, false],
}.freeze
expected_sequences.each do |sequence, expected_state|
  state = ActiveRecord::Base.connection.select_rows("SELECT last_value, is_called FROM #{sequence}").first
  actual_state = [Integer(state.first), ActiveModel::Type::Boolean.new.cast(state.last)]
  raise "#{sequence} state mismatch: #{actual_state.inspect}" unless actual_state == expected_state
end

alice = Account.find(116_844_606_259_201_001)
private_key = OpenSSL::PKey::RSA.new(alice.private_key)
public_key = OpenSSL::PKey::RSA.new(alice.public_key)
raise 'local account RSA keypair does not match' unless private_key.public_key.to_der == public_key.to_der
raise 'local account avatar is not readable from Paperclip' unless alice.avatar.exists?(:original)
raise 'PNG avatar static URL should use original style' unless alice.avatar_static_url == alice.avatar_original_url

remote_accounts = Account.where.not(domain: nil).to_a
raise 'remote account unexpectedly has a private key' unless remote_accounts.all? { |account| account.private_key.nil? }
remote_accounts.each { |account| OpenSSL::PKey::RSA.new(account.public_key) }

remote_keypair = Keypair.find(8901)
raise 'remote keypair unexpectedly has private material' unless remote_keypair.private_key.nil?
OpenSSL::PKey::RSA.new(remote_keypair.public_key)

attachment = MediaAttachment.find(116_844_842_188_806_001)
raise 'status image original is not readable from Paperclip' unless attachment.file.exists?(:original)
raise 'status image small style is not readable from Paperclip' unless attachment.file.exists?(:small)
raise 'status image processing is not complete' unless attachment.processing_complete?
raise 'status image blurhash is missing' if attachment.blurhash.blank?
raise 'status image metadata mismatch' unless attachment.file_meta.deep_symbolize_keys.slice(:original, :small) == {
  original: { width: 600, height: 400, size: '600x400', aspect: 1.5 },
  small: { width: 588, height: 392, size: '588x392', aspect: 1.5 },
}

manifest = JSON.parse(File.read('/fixture/manifest.json'))
manifest.fetch('media').each do |medium|
  path = File.join('/fixture', medium.fetch('path'))
  raise "media dimensions mismatch for #{path}" unless FastImage.size(path).join('x') == medium.fetch('dimensions')
  raise "media byte count mismatch for #{path}" unless File.size(path) == medium.fetch('bytes')
  raise "media hash mismatch for #{path}" unless Digest::SHA256.file(path).hexdigest == medium.fetch('sha256')
end
raise 'avatar file size metadata mismatch' unless alice.avatar_file_size == File.size(alice.avatar.path(:original))
raise 'attachment file size metadata mismatch' unless attachment.file_file_size == File.size(attachment.file.path(:original))

token = Doorkeeper::AccessToken.find_by(token: 'fixture-bearer-token-v4-6-5')
raise 'OAuth bearer token is not readable' if token.nil? || token.revoked? || token.expired?

raise 'visibility mapping mismatch' unless Status.visibilities.values.sort == [0, 1, 2, 3, 4]
raise 'historical poll should be expired' unless Poll.find(8201).expired?
raise 'exclusive list is unreadable' unless List.find(9002).exclusive?
raise 'accepted collection item is unreadable' unless CollectionItem.find(116_845_549_977_608_802).accepted?
raise 'accepted quote is unreadable' unless Quote.find(116_845_314_048_008_702).accepted?

puts 'fixture Rails verification passed: 17 notification types and local media are readable'
