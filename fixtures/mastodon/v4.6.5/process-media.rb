# frozen_string_literal: true

require 'digest'
require 'fastimage'

FIXED_AVATAR_TIME = Time.utc(2026, 7, 1, 12, 0, 0)
FIXED_MEDIA_TIME = Time.utc(2026, 7, 1, 13, 0, 0)

Stoplight.configure { |config| config.data_store = Stoplight::DataStore::Memory.new }

alice = Account.find(116_844_606_259_201_001)
File.open('/fixture/media-source/avatar.png', 'rb') do |source|
  alice.avatar.assign(source)
  alice.avatar.send(:post_process)
  alice.avatar.instance_write(:file_name, '0112603425bb49c1.png')
  alice.avatar.save
end
alice.update_columns(
  avatar_content_type: alice.avatar_content_type,
  avatar_file_name: alice.avatar_file_name,
  avatar_file_size: File.size(alice.avatar.path(:original)),
  avatar_storage_schema_version: alice.avatar_storage_schema_version,
  avatar_updated_at: FIXED_AVATAR_TIME,
  updated_at: FIXED_AVATAR_TIME
)

attachment = MediaAttachment.find(116_844_842_188_806_001)
File.open('/fixture/media-source/status.jpg', 'rb') do |source|
  attachment.file.assign(source)
  attachment.file.send(:post_process)
  attachment.file.instance_write(:file_name, 'cd63911ad76f4d5d.jpg')
  attachment.file.save
end
attachment.update_columns(
  blurhash: attachment.blurhash,
  file_content_type: attachment.file_content_type,
  file_file_name: attachment.file_file_name,
  file_file_size: File.size(attachment.file.path(:original)),
  file_meta: attachment.file_meta,
  file_storage_schema_version: attachment.file_storage_schema_version,
  file_updated_at: FIXED_MEDIA_TIME,
  processing: MediaAttachment.processings.fetch('complete'),
  type: MediaAttachment.types.fetch('image'),
  updated_at: FIXED_MEDIA_TIME
)

expected_files = {
  alice.avatar.path(:original) => [400, 400],
  attachment.file.path(:original) => [600, 400],
  attachment.file.path(:small) => [588, 392],
}.freeze

expected_files.each do |path, dimensions|
  raise "processed media is missing: #{path}" unless File.file?(path)
  raise "processed media dimensions mismatch for #{path}" unless FastImage.size(path) == dimensions
end

raise 'PNG avatar should use original as its static representation' unless alice.avatar_static_url == alice.avatar_original_url
raise 'media processing did not complete' unless attachment.processing_complete?
raise 'media blurhash is missing' if attachment.blurhash.blank?
raise 'media metadata does not cover original and small styles' unless attachment.file_meta.keys.sort == %w(original small)

expected_files.each_key do |path|
  puts "processed #{path.delete_prefix('/fixture/')} sha256=#{Digest::SHA256.file(path).hexdigest}"
end
