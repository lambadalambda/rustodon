-- Owner-only setup in a fresh task restore; all tested mutations occur via UI.
DELETE FROM featured_tags WHERE account_id=116844606259201001;
DELETE FROM tag_follows WHERE account_id=116844606259201001;
DELETE FROM follows WHERE account_id=116844606259201001 AND target_account_id=116844606259202001;
DELETE FROM mutes WHERE account_id=116844606259201001 AND target_account_id=116844606259202001;
DELETE FROM list_accounts WHERE account_id=116844606259202001;
INSERT INTO statuses (id, account_id, text, visibility, local, language, created_at, updated_at, uri, url)
VALUES (119000000000000001,116844606259202001,'Hashtag acceptance home inclusion #FixtureTag',0,false,'en',clock_timestamp(),clock_timestamp(),'https://remote.fixture.invalid/notes/hashtag-browser-8e22f52','https://remote.fixture.invalid/notes/hashtag-browser-8e22f52');
INSERT INTO statuses_tags (status_id,tag_id) VALUES (119000000000000001,9201);
UPDATE statuses SET created_at=clock_timestamp() WHERE id=116844842188805001;
SELECT max(version) AS operational_migration FROM rustodon.schema_migrations;
