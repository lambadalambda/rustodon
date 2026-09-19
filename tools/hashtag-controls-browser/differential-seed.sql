-- Identical owner-only setup in independent Rust/Rails clones, never production.
-- Preserve baseline featured ID 9202 for owner-scoped deletion checks.
DELETE FROM tag_follows WHERE account_id=116844606259201001;
-- Deliberately make current-DB history nonzero without populating Rails Redis.
-- The retained real responses expose, rather than disguise, this exclusion.
-- New chronological status: do not rewrite an older snowflake's timestamp,
-- which would test Rails ID-order vs max(timestamp) behavior instead of controls.
INSERT INTO statuses (id,account_id,text,visibility,local,language,created_at,updated_at)
VALUES (119000000000000001,116844606259201001,'Hashtag differential current public #FixtureTag',0,true,'en',date_trunc('day',current_timestamp),date_trunc('day',current_timestamp));
INSERT INTO statuses_tags (status_id,tag_id) VALUES (119000000000000001,9201);
INSERT INTO oauth_access_tokens (id, application_id, resource_owner_id, token, scopes, created_at) VALUES
(990001,301,101,'hashtag-fixture-write-accounts','write:accounts',clock_timestamp()),
(990002,301,101,'hashtag-fixture-write-follows','write:follows',clock_timestamp()),
(990003,301,107,'hashtag-fixture-other-owner','read write',clock_timestamp()),
(990004,301,NULL,'hashtag-fixture-application','read write',clock_timestamp());
