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

notifications = Notification.where(id: 10_001..10_017, type: expected.keys, filtered: false).order(:id).to_a
raise "expected 17 baseline notifications, found #{notifications.size}" unless notifications.size == expected.size
raise 'known, grouped, legacy, and stress notification rows are incomplete' unless Notification.where(filtered: false).count == 63
unknown_notification = Notification.find(10_018)
raise 'filtered unknown notification fixture mismatch' unless unknown_notification[:type] == 'future_event' && unknown_notification.filtered?
suspended_notification = Notification.find(10_021)
raise 'suspended notification sender fixture mismatch' unless suspended_notification.from_account.suspended?
raise 'suspended sender notification leaked through Mastodon scope' if Notification.where(id: suspended_notification.id).without_suspended.exists?
suspended_request = NotificationRequest.find(-95)
raise 'suspended request sender fixture mismatch' unless suspended_request.from_account.suspended?
raise 'suspended sender request leaked through Mastodon scope' if NotificationRequest.where(id: suspended_request.id).without_suspended.exists?
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
snowflake_records.select! { |record| record.id >= 0 }
snowflake_records.each do |record|
  decoded = Mastodon::Snowflake.to_time(record.id)
  raise "#{record.class} #{record.id} Snowflake timestamp mismatch" unless decoded == record.created_at.utc
end

expected_sequences = {
  'accounts_id_seq' => [6, true],
  'statuses_id_seq' => [14, true],
  'media_attachments_id_seq' => [1, true],
  'quotes_id_seq' => [2, true],
  'collections_id_seq' => [1, true],
  'collection_items_id_seq' => [1, true],
  'notification_requests_id_seq' => [1, true],
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
instance_actor = Account.find(-99)
instance_private_key = OpenSSL::PKey::RSA.new(instance_actor.private_key)
instance_public_key = OpenSSL::PKey::RSA.new(instance_actor.public_key)
raise 'instance actor RSA keypair does not match' unless instance_private_key.public_key.to_der == instance_public_key.to_der
raise 'local account avatar is not readable from Paperclip' unless alice.avatar.exists?(:original)
raise 'PNG avatar static URL should use original style' unless alice.avatar_static_url == alice.avatar_original_url

remote_accounts = Account.where.not(domain: nil).to_a
raise 'remote account unexpectedly has a private key' unless remote_accounts.all? { |account| account.private_key.nil? }
remote_accounts.each { |account| OpenSSL::PKey::RSA.new(account.public_key) }

remote_keypair = Keypair.find(8901)
raise 'remote keypair unexpectedly has private material' unless remote_keypair.private_key.nil?
OpenSSL::PKey::RSA.new(remote_keypair.public_key)

opaque_keypair = Keypair.find(8902)
raise 'encrypted local keypair did not decrypt through Mastodon 4.6.5' unless opaque_keypair.private_key == 'fixture opaque private key material'
raise 'opaque keypair fixture did not retain its raw encrypted envelope' unless opaque_keypair.attributes_before_type_cast['private_key'].start_with?('{"p":')
raise 'opaque preservation-only keypair must stay revoked' unless opaque_keypair.revoked?

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

ordered_media_ids = Status.find(116_844_842_188_805_001).ordered_media_attachments.map(&:id)
raise 'explicit media ordering or limit mismatch' unless ordered_media_ids == [-101, 116_844_842_188_806_001, -102, -103]
fallback_media_ids = Status.find(116_844_846_120_965_002).ordered_media_attachments.map(&:id)
raise 'NULL media ordering fallback mismatch' unless fallback_media_ids == [-210, -209, -208, -207]
expected_descriptions = [nil, 'Deterministic Mastodon test attachment']
raise 'nullable status-edit media descriptions mismatch' unless StatusEdit.find(9403).media_descriptions == expected_descriptions

token = Doorkeeper::AccessToken.find_by(token: 'fixture-bearer-token-v4-6-5')
raise 'OAuth bearer token is not readable' if token.nil? || token.revoked? || token.expired?
granular_status = Doorkeeper::AccessToken.find_by(token: 'fixture-bearer-read-statuses-v4-6-5')
granular_account = Doorkeeper::AccessToken.find_by(token: 'fixture-bearer-read-accounts-v4-6-5')
raise 'granular OAuth status scope is unreadable' unless granular_status&.scopes&.include?('read:statuses')
raise 'granular OAuth account scope is unreadable' unless granular_account&.scopes&.include?('read:accounts')
raise 'revoked OAuth fixture is not revoked' unless Doorkeeper::AccessToken.find_by(token: 'fixture-bearer-revoked-v4-6-5')&.revoked?
raise 'expired OAuth fixture is not expired' unless Doorkeeper::AccessToken.find_by(token: 'fixture-bearer-expired-v4-6-5')&.expired?
application_only = Doorkeeper::AccessToken.find_by(token: 'fixture-bearer-application-only-v4-6-5')
raise 'application-only OAuth fixture has a resource owner' unless application_only&.resource_owner_id.nil?
raise 'disabled-user OAuth fixture is not linked exactly' unless Doorkeeper::AccessToken.find_by(token: 'fixture-bearer-disabled-user-v4-6-5')&.resource_owner_id == 103
raise 'missing-2FA OAuth fixture is not linked exactly' unless Doorkeeper::AccessToken.find_by(token: 'fixture-bearer-missing-2fa-v4-6-5')&.resource_owner_id == 102
api_moderator_token = Doorkeeper::AccessToken.find_by(token: 'fixture-bearer-api-moderator-v4-6-5')
raise 'functional API moderator OAuth fixture is not linked exactly' unless api_moderator_token&.resource_owner_id == 104 && User.find(104).functional?
matrix_viewer_token = Doorkeeper::AccessToken.find_by(token: 'fixture-bearer-matrix-viewer-v4-6-5')
raise 'functional matrix-viewer OAuth fixture is not linked exactly' unless matrix_viewer_token&.resource_owner_id == 107 && User.find(107).functional?
raise 'pending local account fixture is not pending' unless User.find(105).pending?
raise 'unconfirmed local account fixture is confirmed' if User.find(106).confirmed?

raise 'visibility mapping mismatch' unless Status.visibilities.values.sort == [0, 1, 2, 3, 4]
raise 'historical poll should be expired' unless Poll.find(8201).expired?
raise 'exclusive list is unreadable' unless List.find(9002).exclusive?
raise 'accepted collection item is unreadable' unless CollectionItem.find(116_845_549_977_608_802).accepted?
raise 'accepted quote is unreadable' unless Quote.find(116_845_314_048_008_702).accepted?
deleted_target_quote = Quote.find(-94)
raise 'deleted-target quote state is unreadable' unless deleted_target_quote.deleted?
raise 'soft-deleted quoted status should be hidden by default' unless deleted_target_quote.quoted_status.nil?

raise 'instance actor should be local without a user' unless Account.find(-99).local? && Account.find(-99).user.nil?
raise 'soft-deleted unknown visibility fixture should be hidden by default' if Status.exists?(116_846_257_766_400_501)
unknown_visibility_status = Status.unscoped.find(116_846_257_766_400_501)
raise 'soft-deleted status is missing from unscoped raw reads' unless unknown_visibility_status.attributes_before_type_cast['visibility'] == 99
raise 'scalar Rails YAML setting is unreadable' unless Setting.find(9801).value == true
tagged_setting = Setting.find(9802).value
raise 'tagged Rails YAML setting is unreadable' unless tagged_setting.is_a?(ActiveSupport::HashWithIndifferentAccess) && tagged_setting[:fixture] == 'value'

enriched_status = Status.find(116_845_105_643_525_105)
raise 'status custom emoji is unreadable' unless enriched_status.emojis.map(&:shortcode) == ['fixtureparty']
raise 'status preview card is unreadable' unless enriched_status.preview_card&.id == 12_002
raise 'status tagged collection is unreadable' unless enriched_status.tagged_objects.filter_map(&:object).map(&:id) == [116_845_549_977_608_801]
raise 'home marker is unreadable' unless Marker.find(12_004).last_read_id == 116_844_842_188_805_001
raise 'grouped favourite notification fixture is incomplete' unless Notification.where(group_key: 'favourite-116844842188805001-495255').count == 2
raise 'legacy null-type notification mapping is unreadable' unless Notification.find(10_025).type == :reblog
raise 'notification grouping stress fixture is incomplete' unless Notification.where(group_key: 'follow-api-moderator-stress').count == 41

puts 'fixture Rails verification passed: notifications, suspended senders, status edits, and local media are readable'
