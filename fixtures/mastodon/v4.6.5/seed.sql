\set ON_ERROR_STOP on

DO $$
BEGIN
  IF current_database() <> 'rustodon_mastodon_v4_6_5_fixture' THEN
    RAISE EXCEPTION 'refusing to seed non-fixture database: %', current_database();
  END IF;
END
$$;

BEGIN;

SET TIME ZONE 'UTC';

UPDATE ar_internal_metadata
SET created_at = '2026-07-01 00:00:00', updated_at = '2026-07-01 00:00:00'
WHERE key IN ('environment', 'schema_sha1');

-- Mastodon normally generates this salt at schema-load time. The fixture salt
-- is fixed so pg_dump and the catalog function fingerprint are reproducible.
CREATE OR REPLACE FUNCTION public.timestamp_id(table_name text)
RETURNS bigint AS
$$
  DECLARE
    time_part bigint;
    sequence_base bigint;
    tail bigint;
  BEGIN
    time_part := (((date_part('epoch', now()) * 1000))::bigint << 16);
    sequence_base := (
      'x' || substr(
        md5(table_name || 'rustodon-mastodon-v4.6.5-fixture-salt' || time_part::text),
        1,
        4
      )
    )::bit(16)::bigint;
    tail := ((sequence_base + nextval(table_name || '_id_seq')) & 65535);
    RETURN time_part | tail;
  END
$$ LANGUAGE plpgsql VOLATILE;

INSERT INTO accounts (
  id, username, domain, display_name, note, uri, url, actor_type,
  public_key, private_key, inbox_url, outbox_url, followers_url,
  following_url, shared_inbox_url, featured_collection_url, collections_url,
  locked, discoverable, indexable, avatar_file_name, avatar_content_type,
  avatar_file_size, avatar_updated_at, avatar_storage_schema_version,
  avatar_description, created_at, updated_at
) VALUES
  (
    116844606259201001, 'alice', NULL, 'Alice Fixture', 'Primary local fixture account', '',
    'https://fixture-v4-6-5.rustodon.invalid/@alice', 'Person',
    $fixture_public$-----BEGIN PUBLIC KEY-----
MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEAqIAYvNFGbZ5g4iiK6feS
dXD4bDStFM58A7tHycYXaYtzZQpIeHXAmaXuZzXIwtrP4N0gIk8JNwZvXj2UPS+S
07t0V9wNK94he01LV5EMz/GN4eNnFmDL64HIEuKLvV8TvgjbUPRD6Y5X0UpKi2ZI
FLSb96Q5w0Z/k7ntpVKV52y8kz5Fjr/O/0JuHryZe0yItzJh8kzFfeMf0EXzfSna
KvT7P9jhgC6uTre+jXyvVZjiHDrnqvvucdI3I7DRfXo1OqARBrLjy+TdseUAjNYJ
+OuPRI1URIWQI01DCHqcohVu9+Ar+BiCjFp3ua+XMuJvrvbD61d1Fvig/9nbBRR+
8QIDAQAB
-----END PUBLIC KEY-----
$fixture_public$,
    $fixture_private$-----BEGIN RSA PRIVATE KEY-----
MIIEowIBAAKCAQEAqIAYvNFGbZ5g4iiK6feSdXD4bDStFM58A7tHycYXaYtzZQpI
eHXAmaXuZzXIwtrP4N0gIk8JNwZvXj2UPS+S07t0V9wNK94he01LV5EMz/GN4eNn
FmDL64HIEuKLvV8TvgjbUPRD6Y5X0UpKi2ZIFLSb96Q5w0Z/k7ntpVKV52y8kz5F
jr/O/0JuHryZe0yItzJh8kzFfeMf0EXzfSnaKvT7P9jhgC6uTre+jXyvVZjiHDrn
qvvucdI3I7DRfXo1OqARBrLjy+TdseUAjNYJ+OuPRI1URIWQI01DCHqcohVu9+Ar
+BiCjFp3ua+XMuJvrvbD61d1Fvig/9nbBRR+8QIDAQABAoIBAAgySHnFWI6gItR3
fkfiqIm80cHCN3Xk1C6iiVu+3oBOZbHpW9R7vl9e/WOA/9O+LPjiSsQOegtWnVvd
RRjrl7Hj20VDlZKv5Mssm6zOGAxksrcVbqwdj+fUJaNJCL0AyyseH0x/IE9T8rDC
I1GH+3tB3JkhkIN/qjipdX5ab8MswEPu8IC4ViTpdBgWYY/xBcAHPw4xuL0tcwzh
FBlf4DqoEVQo8GdK5GAJ2Ny0S4xbXHUURzx/R4y4CCts7niAiLGqd9jmLU1kUTMk
QcXfQYK6l+unLc7wDYAz7sFEHh04M48VjWwiIZJnlCqmQbLda7uhhu8zkF1DqZTu
ulWDGQECgYEA0TIAc8BQBVab979DHEEmMdgqBwxLY3OIAk0b+r50h7VBGWCDPRsC
STD73fQY3lNet/7/jgSGwwAlAJ5PpMXxXiZAE3bUwPmHzgF7pvIOOLhA8O07tHSO
L2mvQe6NPzjZ+6iAO2U9PkClxcvGvPx2OBvisfHqZLmxC9PIVxzruQECgYEAzjM6
BTUXa6T/qHvLFbN699BXsUOGmHBGaLRapFDBfVvgZrwqYQcZpBBhesLdGTGSqwE7
gWsITPIJ+Ldo+38oGYyVys+w/V67q6ud7hgSDTW3hSvm+GboCjk6gzxlt9hQ0t9X
8vfDOYhEXvVUJNv3mYO60ENqQhILO4bQ0zi+VfECgYBb/nUccfG+pzunU0Cb6Dp3
qOuydcGhVmj1OhuXxLFSDG84Tazo7juvHA9mp7VX76mzmDuhpHPuxN2AzB2SBEoE
cSW0aYld413JRfWukLuYTc6hJHIhBTCRwRQFFnae2s1hUdQySm8INT2xIc+fxBXo
zrp+Ljg5Wz90SAnN5TX0AQKBgDaatDOq0o/r+tPYLHiLtfWoE4Dau+rkWJDjqdk3
lXWn/e3WyHY3Vh/vQpEqxzgju45TXjmwaVtPATr+/usSykCxzP0PMPR3wMT+Rm1F
rIoY/odij+CaB7qlWwxj0x/zRbwB7x1lZSp4HnrzBpxYL+JUUwVRxPLIKndSBTza
GvVRAoGBAIVBcNcRQYF4fvZjDKAb4fdBsEuHmycqtRCsnkGOz6ebbEQznSaZ0tZE
+JuouZaGjyp8uPjNGD5D7mIGbyoZ3KyG4mTXNxDAGBso1hrNDKGBOrGaPhZx8LgO
4VXJ+ybXrATf4jr8ccZYsZdFpOphPzz+j55Mqg5vac5P1XjmsGTb
-----END RSA PRIVATE KEY-----
$fixture_private$,
    '', '', '', '', '', NULL, NULL, true, true, true,
    NULL, NULL, NULL, NULL, NULL,
    'Deterministic Mastodon test avatar',
    '2026-07-01 12:00:00', '2026-07-01 12:00:00'
  ),
  (
    116844606259201002, 'moderator', NULL, 'Moderator Fixture', 'Local moderator fixture', '',
    'https://fixture-v4-6-5.rustodon.invalid/@moderator', 'Person',
    $fixture_public$-----BEGIN PUBLIC KEY-----
MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEAqIAYvNFGbZ5g4iiK6feS
dXD4bDStFM58A7tHycYXaYtzZQpIeHXAmaXuZzXIwtrP4N0gIk8JNwZvXj2UPS+S
07t0V9wNK94he01LV5EMz/GN4eNnFmDL64HIEuKLvV8TvgjbUPRD6Y5X0UpKi2ZI
FLSb96Q5w0Z/k7ntpVKV52y8kz5Fjr/O/0JuHryZe0yItzJh8kzFfeMf0EXzfSna
KvT7P9jhgC6uTre+jXyvVZjiHDrnqvvucdI3I7DRfXo1OqARBrLjy+TdseUAjNYJ
+OuPRI1URIWQI01DCHqcohVu9+Ar+BiCjFp3ua+XMuJvrvbD61d1Fvig/9nbBRR+
8QIDAQAB
-----END PUBLIC KEY-----
$fixture_public$,
    $fixture_private$-----BEGIN RSA PRIVATE KEY-----
MIIEowIBAAKCAQEAqIAYvNFGbZ5g4iiK6feSdXD4bDStFM58A7tHycYXaYtzZQpI
eHXAmaXuZzXIwtrP4N0gIk8JNwZvXj2UPS+S07t0V9wNK94he01LV5EMz/GN4eNn
FmDL64HIEuKLvV8TvgjbUPRD6Y5X0UpKi2ZIFLSb96Q5w0Z/k7ntpVKV52y8kz5F
jr/O/0JuHryZe0yItzJh8kzFfeMf0EXzfSnaKvT7P9jhgC6uTre+jXyvVZjiHDrn
qvvucdI3I7DRfXo1OqARBrLjy+TdseUAjNYJ+OuPRI1URIWQI01DCHqcohVu9+Ar
+BiCjFp3ua+XMuJvrvbD61d1Fvig/9nbBRR+8QIDAQABAoIBAAgySHnFWI6gItR3
fkfiqIm80cHCN3Xk1C6iiVu+3oBOZbHpW9R7vl9e/WOA/9O+LPjiSsQOegtWnVvd
RRjrl7Hj20VDlZKv5Mssm6zOGAxksrcVbqwdj+fUJaNJCL0AyyseH0x/IE9T8rDC
I1GH+3tB3JkhkIN/qjipdX5ab8MswEPu8IC4ViTpdBgWYY/xBcAHPw4xuL0tcwzh
FBlf4DqoEVQo8GdK5GAJ2Ny0S4xbXHUURzx/R4y4CCts7niAiLGqd9jmLU1kUTMk
QcXfQYK6l+unLc7wDYAz7sFEHh04M48VjWwiIZJnlCqmQbLda7uhhu8zkF1DqZTu
ulWDGQECgYEA0TIAc8BQBVab979DHEEmMdgqBwxLY3OIAk0b+r50h7VBGWCDPRsC
STD73fQY3lNet/7/jgSGwwAlAJ5PpMXxXiZAE3bUwPmHzgF7pvIOOLhA8O07tHSO
L2mvQe6NPzjZ+6iAO2U9PkClxcvGvPx2OBvisfHqZLmxC9PIVxzruQECgYEAzjM6
BTUXa6T/qHvLFbN699BXsUOGmHBGaLRapFDBfVvgZrwqYQcZpBBhesLdGTGSqwE7
gWsITPIJ+Ldo+38oGYyVys+w/V67q6ud7hgSDTW3hSvm+GboCjk6gzxlt9hQ0t9X
8vfDOYhEXvVUJNv3mYO60ENqQhILO4bQ0zi+VfECgYBb/nUccfG+pzunU0Cb6Dp3
qOuydcGhVmj1OhuXxLFSDG84Tazo7juvHA9mp7VX76mzmDuhpHPuxN2AzB2SBEoE
cSW0aYld413JRfWukLuYTc6hJHIhBTCRwRQFFnae2s1hUdQySm8INT2xIc+fxBXo
zrp+Ljg5Wz90SAnN5TX0AQKBgDaatDOq0o/r+tPYLHiLtfWoE4Dau+rkWJDjqdk3
lXWn/e3WyHY3Vh/vQpEqxzgju45TXjmwaVtPATr+/usSykCxzP0PMPR3wMT+Rm1F
rIoY/odij+CaB7qlWwxj0x/zRbwB7x1lZSp4HnrzBpxYL+JUUwVRxPLIKndSBTza
GvVRAoGBAIVBcNcRQYF4fvZjDKAb4fdBsEuHmycqtRCsnkGOz6ebbEQznSaZ0tZE
+JuouZaGjyp8uPjNGD5D7mIGbyoZ3KyG4mTXNxDAGBso1hrNDKGBOrGaPhZx8LgO
4VXJ+ybXrATf4jr8ccZYsZdFpOphPzz+j55Mqg5vac5P1XjmsGTb
-----END RSA PRIVATE KEY-----
$fixture_private$,
    '', '', '', '', '', NULL, NULL, false, false, false,
    NULL, NULL, NULL, NULL, NULL, '',
    '2026-07-01 12:00:00', '2026-07-01 12:00:00'
  ),
  (
    116844606259201003, 'newbie', NULL, 'New User Fixture', 'Local sign-up notification actor', '',
    'https://fixture-v4-6-5.rustodon.invalid/@newbie', 'Person',
    $fixture_public$-----BEGIN PUBLIC KEY-----
MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEAqIAYvNFGbZ5g4iiK6feS
dXD4bDStFM58A7tHycYXaYtzZQpIeHXAmaXuZzXIwtrP4N0gIk8JNwZvXj2UPS+S
07t0V9wNK94he01LV5EMz/GN4eNnFmDL64HIEuKLvV8TvgjbUPRD6Y5X0UpKi2ZI
FLSb96Q5w0Z/k7ntpVKV52y8kz5Fjr/O/0JuHryZe0yItzJh8kzFfeMf0EXzfSna
KvT7P9jhgC6uTre+jXyvVZjiHDrnqvvucdI3I7DRfXo1OqARBrLjy+TdseUAjNYJ
+OuPRI1URIWQI01DCHqcohVu9+Ar+BiCjFp3ua+XMuJvrvbD61d1Fvig/9nbBRR+
8QIDAQAB
-----END PUBLIC KEY-----
$fixture_public$,
    $fixture_private$-----BEGIN RSA PRIVATE KEY-----
MIIEowIBAAKCAQEAqIAYvNFGbZ5g4iiK6feSdXD4bDStFM58A7tHycYXaYtzZQpI
eHXAmaXuZzXIwtrP4N0gIk8JNwZvXj2UPS+S07t0V9wNK94he01LV5EMz/GN4eNn
FmDL64HIEuKLvV8TvgjbUPRD6Y5X0UpKi2ZIFLSb96Q5w0Z/k7ntpVKV52y8kz5F
jr/O/0JuHryZe0yItzJh8kzFfeMf0EXzfSnaKvT7P9jhgC6uTre+jXyvVZjiHDrn
qvvucdI3I7DRfXo1OqARBrLjy+TdseUAjNYJ+OuPRI1URIWQI01DCHqcohVu9+Ar
+BiCjFp3ua+XMuJvrvbD61d1Fvig/9nbBRR+8QIDAQABAoIBAAgySHnFWI6gItR3
fkfiqIm80cHCN3Xk1C6iiVu+3oBOZbHpW9R7vl9e/WOA/9O+LPjiSsQOegtWnVvd
RRjrl7Hj20VDlZKv5Mssm6zOGAxksrcVbqwdj+fUJaNJCL0AyyseH0x/IE9T8rDC
I1GH+3tB3JkhkIN/qjipdX5ab8MswEPu8IC4ViTpdBgWYY/xBcAHPw4xuL0tcwzh
FBlf4DqoEVQo8GdK5GAJ2Ny0S4xbXHUURzx/R4y4CCts7niAiLGqd9jmLU1kUTMk
QcXfQYK6l+unLc7wDYAz7sFEHh04M48VjWwiIZJnlCqmQbLda7uhhu8zkF1DqZTu
ulWDGQECgYEA0TIAc8BQBVab979DHEEmMdgqBwxLY3OIAk0b+r50h7VBGWCDPRsC
STD73fQY3lNet/7/jgSGwwAlAJ5PpMXxXiZAE3bUwPmHzgF7pvIOOLhA8O07tHSO
L2mvQe6NPzjZ+6iAO2U9PkClxcvGvPx2OBvisfHqZLmxC9PIVxzruQECgYEAzjM6
BTUXa6T/qHvLFbN699BXsUOGmHBGaLRapFDBfVvgZrwqYQcZpBBhesLdGTGSqwE7
gWsITPIJ+Ldo+38oGYyVys+w/V67q6ud7hgSDTW3hSvm+GboCjk6gzxlt9hQ0t9X
8vfDOYhEXvVUJNv3mYO60ENqQhILO4bQ0zi+VfECgYBb/nUccfG+pzunU0Cb6Dp3
qOuydcGhVmj1OhuXxLFSDG84Tazo7juvHA9mp7VX76mzmDuhpHPuxN2AzB2SBEoE
cSW0aYld413JRfWukLuYTc6hJHIhBTCRwRQFFnae2s1hUdQySm8INT2xIc+fxBXo
zrp+Ljg5Wz90SAnN5TX0AQKBgDaatDOq0o/r+tPYLHiLtfWoE4Dau+rkWJDjqdk3
lXWn/e3WyHY3Vh/vQpEqxzgju45TXjmwaVtPATr+/usSykCxzP0PMPR3wMT+Rm1F
rIoY/odij+CaB7qlWwxj0x/zRbwB7x1lZSp4HnrzBpxYL+JUUwVRxPLIKndSBTza
GvVRAoGBAIVBcNcRQYF4fvZjDKAb4fdBsEuHmycqtRCsnkGOz6ebbEQznSaZ0tZE
+JuouZaGjyp8uPjNGD5D7mIGbyoZ3KyG4mTXNxDAGBso1hrNDKGBOrGaPhZx8LgO
4VXJ+ybXrATf4jr8ccZYsZdFpOphPzz+j55Mqg5vac5P1XjmsGTb
-----END RSA PRIVATE KEY-----
$fixture_private$,
    '', '', '', '', '', NULL, NULL, false, true, true,
    NULL, NULL, NULL, NULL, NULL, '',
    '2026-07-01 12:00:00', '2026-07-01 12:00:00'
  ),
  (
    116844606259202001, 'bob', 'remote.fixture.invalid', 'Bob Remote', 'Primary remote fixture actor',
    'https://remote.fixture.invalid/users/bob',
    'https://remote.fixture.invalid/@bob', 'Person',
    $fixture_public$-----BEGIN PUBLIC KEY-----
MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEAqIAYvNFGbZ5g4iiK6feS
dXD4bDStFM58A7tHycYXaYtzZQpIeHXAmaXuZzXIwtrP4N0gIk8JNwZvXj2UPS+S
07t0V9wNK94he01LV5EMz/GN4eNnFmDL64HIEuKLvV8TvgjbUPRD6Y5X0UpKi2ZI
FLSb96Q5w0Z/k7ntpVKV52y8kz5Fjr/O/0JuHryZe0yItzJh8kzFfeMf0EXzfSna
KvT7P9jhgC6uTre+jXyvVZjiHDrnqvvucdI3I7DRfXo1OqARBrLjy+TdseUAjNYJ
+OuPRI1URIWQI01DCHqcohVu9+Ar+BiCjFp3ua+XMuJvrvbD61d1Fvig/9nbBRR+
8QIDAQAB
-----END PUBLIC KEY-----
$fixture_public$,
    NULL,
    'https://remote.fixture.invalid/users/bob/inbox',
    'https://remote.fixture.invalid/users/bob/outbox',
    'https://remote.fixture.invalid/users/bob/followers',
    'https://remote.fixture.invalid/users/bob/following',
    'https://remote.fixture.invalid/inbox',
    'https://remote.fixture.invalid/users/bob/collections/featured',
    'https://remote.fixture.invalid/users/bob/collections',
    false, true, true, NULL, NULL, NULL, NULL, NULL, '',
    '2026-07-01 12:00:00', '2026-07-01 12:00:00'
  ),
  (
    116844606259202002, 'carol', 'remote.fixture.invalid', 'Carol Remote', 'Remote pending follower',
    'https://remote.fixture.invalid/users/carol',
    'https://remote.fixture.invalid/@carol', 'Person',
    $fixture_public$-----BEGIN PUBLIC KEY-----
MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEAqIAYvNFGbZ5g4iiK6feS
dXD4bDStFM58A7tHycYXaYtzZQpIeHXAmaXuZzXIwtrP4N0gIk8JNwZvXj2UPS+S
07t0V9wNK94he01LV5EMz/GN4eNnFmDL64HIEuKLvV8TvgjbUPRD6Y5X0UpKi2ZI
FLSb96Q5w0Z/k7ntpVKV52y8kz5Fjr/O/0JuHryZe0yItzJh8kzFfeMf0EXzfSna
KvT7P9jhgC6uTre+jXyvVZjiHDrnqvvucdI3I7DRfXo1OqARBrLjy+TdseUAjNYJ
+OuPRI1URIWQI01DCHqcohVu9+Ar+BiCjFp3ua+XMuJvrvbD61d1Fvig/9nbBRR+
8QIDAQAB
-----END PUBLIC KEY-----
$fixture_public$,
    NULL,
    'https://remote.fixture.invalid/users/carol/inbox',
    'https://remote.fixture.invalid/users/carol/outbox',
    'https://remote.fixture.invalid/users/carol/followers',
    'https://remote.fixture.invalid/users/carol/following',
    'https://remote.fixture.invalid/inbox', NULL, NULL,
    false, true, true, NULL, NULL, NULL, NULL, NULL, '',
    '2026-07-01 12:00:00', '2026-07-01 12:00:00'
  ),
  (
    -99, 'fixture-v4-6-5.rustodon.invalid', NULL, 'Fixture Instance Actor',
    'Local service actor without a login-capable user',
    'https://fixture-v4-6-5.rustodon.invalid/actor',
    'https://fixture-v4-6-5.rustodon.invalid/actor', 'Application',
    '', NULL,
    'https://fixture-v4-6-5.rustodon.invalid/actor/inbox',
    'https://fixture-v4-6-5.rustodon.invalid/actor/outbox',
    'https://fixture-v4-6-5.rustodon.invalid/actor/followers',
    'https://fixture-v4-6-5.rustodon.invalid/actor/following',
    'https://fixture-v4-6-5.rustodon.invalid/inbox', NULL, NULL,
    false, false, false, NULL, NULL, NULL, NULL, NULL, '',
    '2026-07-01 12:00:00', '2026-07-01 12:00:00'
  );

UPDATE accounts
SET also_known_as = ARRAY['https://alias.remote.fixture.invalid/users/bob'],
    attribution_domains = ARRAY['media.remote.fixture.invalid'],
    fields = '[{"name":"Fixture field","value":"Exact JSONB value","verified_at":null}]'::jsonb
WHERE id = 116844606259202001;

UPDATE accounts SET attribution_domains = NULL WHERE id = -99;
UPDATE accounts SET id_scheme = 0 WHERE id = 116844606259201001;

INSERT INTO accounts (
  id, username, domain, display_name, note, uri, url, actor_type,
  public_key, private_key, inbox_url, outbox_url, followers_url,
  following_url, shared_inbox_url, protocol, id_scheme, suspended_at,
  created_at, updated_at
)
SELECT
  116844606259202003, 'suspended', 'remote.fixture.invalid',
  'Suspended Remote Fixture', 'Suspended sender exclusion coverage',
  'https://remote.fixture.invalid/users/suspended',
  'https://remote.fixture.invalid/@suspended', 'Person', public_key, NULL,
  'https://remote.fixture.invalid/users/suspended/inbox',
  'https://remote.fixture.invalid/users/suspended/outbox',
  'https://remote.fixture.invalid/users/suspended/followers',
  'https://remote.fixture.invalid/users/suspended/following',
  'https://remote.fixture.invalid/inbox', 1, 1, '2026-07-01 18:30:00',
  '2026-07-01 12:00:00', '2026-07-01 18:30:00'
FROM accounts
WHERE id = 116844606259202001;

INSERT INTO user_roles (
  id, name, color, position, permissions, highlighted, require_2fa,
  collection_limit, created_at, updated_at
) VALUES
  (-99, '', '', -1, 1152921504606912512, false, false, 10, '2026-07-01 12:00:00', '2026-07-01 12:00:00'),
  (91, 'Fixture user', '', 0, 0, false, true, 10, '2026-07-01 12:00:00', '2026-07-01 12:00:00'),
  (92, 'Fixture moderator', '1d9bf0', 10, 1049616, true, true, 10, '2026-07-01 12:00:00', '2026-07-01 12:00:00');

INSERT INTO users (
  id, account_id, email, encrypted_password, confirmed_at, approved, disabled,
  locale, chosen_languages, otp_backup_codes, settings, role_id, sign_up_ip,
  created_at, updated_at
) VALUES
  (101, 116844606259201001, 'alice@fixture.invalid', '$2a$04$eYtbMaSJeYOgS7ENqTs6vezGAQVltj68iGTiHiIsdst5LyirUp5JC', '2026-07-01 12:00:00', true, false, 'en', ARRAY['en'], ARRAY['fixture-recovery-code'], '{"default_privacy":"private","nested":{"number":9007199254740993}}', 91, '192.0.2.0/24', '2026-07-01 12:00:00', '2026-07-01 12:00:00'),
  (102, 116844606259201002, 'moderator@fixture.invalid', '$2a$04$eYtbMaSJeYOgS7ENqTs6vezGAQVltj68iGTiHiIsdst5LyirUp5JC', '2026-07-01 12:00:00', true, false, 'en', ARRAY[]::varchar[], ARRAY[]::varchar[], NULL, 92, '192.0.2.11', '2026-07-01 12:00:00', '2026-07-01 12:00:00'),
  (103, 116844606259201003, 'newbie@fixture.invalid', '$2a$04$eYtbMaSJeYOgS7ENqTs6vezGAQVltj68iGTiHiIsdst5LyirUp5JC', '2026-07-01 12:00:00', true, true, 'en', ARRAY['en', 'fr'], NULL, '', 91, '192.0.2.12', '2026-07-01 12:00:00', '2026-07-01 12:00:00');

UPDATE users SET webauthn_id = 'fixture-alice-webauthn-id' WHERE id = 101;

INSERT INTO webauthn_credentials (
  id, user_id, external_id, nickname, public_key, sign_count, created_at, updated_at
) VALUES (
  1, 101, 'fixture-alice-webauthn-credential', 'Fixture security key',
  'fixture-public-credential-key', 0,
  '2026-07-01 12:00:00', '2026-07-01 12:00:00'
);

INSERT INTO oauth_applications (
  id, name, uid, secret, redirect_uri, scopes, confidential, owner_id,
  owner_type, website, created_at, updated_at
) VALUES (
  301, 'Rustodon fixture client', 'rustodon-fixture-client-v4-6-5',
  'fixture-only-client-secret-v4-6-5', 'urn:ietf:wg:oauth:2.0:oob',
  'read write follow push', true, 101, 'User',
  'https://fixture-client.invalid', '2026-07-01 12:00:00', '2026-07-01 12:00:00'
);

INSERT INTO oauth_access_tokens (
  id, resource_owner_id, application_id, token, refresh_token, scopes,
  created_at, last_used_at, last_used_ip
) VALUES (
  401, 101, 301, 'fixture-bearer-token-v4-6-5',
  'fixture-refresh-token-v4-6-5', 'read write follow push',
  '2026-07-01 12:00:00', '2026-07-01 12:30:00', '192.0.2.10'
);

INSERT INTO statuses (
  id, account_id, text, spoiler_text, visibility, local, uri, url, language,
  sensitive, reply, ordered_media_attachment_ids, edited_at, created_at, updated_at
) VALUES
  (116844842188805001, 116844606259201001, 'Public fixture status with local media', '', 0, true, NULL, NULL, 'en', false, false, ARRAY[-101, 116844842188806001, -102, -103, -104]::bigint[], NULL, '2026-07-01 13:00:00', '2026-07-01 13:00:00'),
  (116844846120965002, 116844606259201001, 'Unlisted fixture status', '', 1, true, NULL, NULL, 'en', false, false, NULL, NULL, '2026-07-01 13:01:00', '2026-07-01 13:01:00'),
  (116844850053125003, 116844606259201001, 'Followers-only fixture status', '', 2, true, NULL, NULL, 'en', false, false, NULL, NULL, '2026-07-01 13:02:00', '2026-07-01 13:02:00'),
  (116844853985285004, 116844606259201001, 'Direct fixture status for Bob', '', 3, true, NULL, NULL, 'en', false, false, NULL, NULL, '2026-07-01 13:03:00', '2026-07-01 13:03:00'),
  (116844857917445005, 116844606259201001, 'Limited fixture status', '', 4, true, NULL, NULL, 'en', false, false, NULL, NULL, '2026-07-01 13:04:00', '2026-07-01 13:04:00'),
  (116845078118405101, 116844606259202001, 'Remote mention of @alice', '', 0, false, 'https://remote.fixture.invalid/users/bob/statuses/116845078118405101', 'https://remote.fixture.invalid/@bob/116845078118405101', 'en', false, false, NULL, NULL, '2026-07-01 14:00:00', '2026-07-01 14:00:00'),
  (111680579174405102, 116844606259202001, 'Historical fixture poll', '', 0, false, 'https://remote.fixture.invalid/users/bob/statuses/111680579174405102', 'https://remote.fixture.invalid/@bob/111680579174405102', 'en', false, false, NULL, NULL, '2024-01-01 12:00:00', '2024-01-02 12:00:00'),
  (116845093847045103, 116844606259202001, 'Remote status quoted by Alice', '', 0, false, 'https://remote.fixture.invalid/users/bob/statuses/116845093847045103', 'https://remote.fixture.invalid/@bob/116845093847045103', 'en', false, false, NULL, '2026-07-01 14:05:00', '2026-07-01 14:04:00', '2026-07-01 14:05:00'),
  (116845101711365104, 116844606259202001, 'New status notification activity', '', 0, false, 'https://remote.fixture.invalid/users/bob/statuses/116845101711365104', 'https://remote.fixture.invalid/@bob/116845101711365104', 'en', false, false, NULL, NULL, '2026-07-01 14:06:00', '2026-07-01 14:06:00'),
  (116845105643525105, 116844606259202001, 'Edited remote status notification activity', '', 0, false, 'https://remote.fixture.invalid/users/bob/statuses/116845105643525105', 'https://remote.fixture.invalid/@bob/116845105643525105', 'en', false, false, NULL, '2026-07-01 14:08:00', '2026-07-01 14:07:00', '2026-07-01 14:08:00'),
  (116845314048005201, 116844606259201001, 'Alice quotes Bob for quoted-update coverage', '', 0, true, NULL, NULL, 'en', false, false, NULL, NULL, '2026-07-01 15:00:00', '2026-07-01 15:00:00'),
  (116845317980165202, 116844606259202001, 'Bob quotes Alice for quote coverage', '', 0, false, 'https://remote.fixture.invalid/users/bob/statuses/116845317980165202', 'https://remote.fixture.invalid/@bob/116845317980165202', 'en', false, false, NULL, NULL, '2026-07-01 15:01:00', '2026-07-01 15:01:00'),
  (116845321912325301, 116844606259202001, '', '', 0, false, 'https://remote.fixture.invalid/users/bob/statuses/116845321912325301/activity', 'https://remote.fixture.invalid/@bob/116845321912325301', NULL, false, false, NULL, NULL, '2026-07-01 15:02:00', '2026-07-01 15:02:00');

INSERT INTO statuses (
  id, account_id, text, spoiler_text, visibility, local, uri, url, language,
  sensitive, reply, ordered_media_attachment_ids, deleted_at, created_at, updated_at
) VALUES (
  116846257766400501, 116844606259201001,
  'Soft-deleted unknown visibility fixture status', '', 99, true, NULL, NULL, 'en',
  false, false, ARRAY[]::bigint[], '2026-07-01 19:30:00',
  '2026-07-01 19:00:00', '2026-07-01 19:30:00'
);

UPDATE statuses SET reblog_of_id = 116844842188805001 WHERE id = 116845321912325301;
UPDATE statuses SET application_id = 301, quote_approval_policy = 2 WHERE id = 116844842188805001;
UPDATE statuses
SET in_reply_to_account_id = 116844606259202001,
    in_reply_to_id = 116845078118405101,
    reply = true
WHERE id = 116844853985285004;

INSERT INTO tags (
  id, name, display_name, usable, trendable, listable, last_status_at,
  created_at, updated_at
) VALUES (
  9201, 'fixturetag', 'FixtureTag', true, false, true, '2026-07-01 13:00:00',
  '2026-07-01 13:00:00', '2026-07-01 13:00:00'
);

INSERT INTO statuses_tags (status_id, tag_id) VALUES
  (116844842188805001, 9201),
  (116846257766400501, 9201);

INSERT INTO accounts_tags (account_id, tag_id) VALUES (116844606259201001, 9201);

INSERT INTO featured_tags (
  id, account_id, tag_id, name, statuses_count, last_status_at, created_at, updated_at
) VALUES (
  9202, 116844606259201001, 9201, 'fixturetag', 1, '2026-07-01 13:00:00',
  '2026-07-01 13:00:00', '2026-07-01 13:00:00'
);

INSERT INTO conversations (
  id, uri, parent_account_id, parent_status_id, created_at, updated_at
) VALUES (
  9301, 'https://fixture-v4-6-5.rustodon.invalid/conversations/9301',
  116844606259201001, 116844853985285004,
  '2026-07-01 13:03:00', '2026-07-01 13:03:00'
);

UPDATE statuses SET conversation_id = 9301 WHERE id = 116844853985285004;

INSERT INTO account_conversations (
  id, account_id, conversation_id, last_status_id, participant_account_ids,
  status_ids, unread
) VALUES (
  9302, 116844606259201001, 9301, 116844853985285004,
  ARRAY[116844606259202001]::bigint[],
  ARRAY[116844853985285004, 116846257766400501]::bigint[], true
);

INSERT INTO conversation_mutes (id, account_id, conversation_id) VALUES (
  9303, 116844606259201001, 9301
);

INSERT INTO status_edits (
  id, account_id, status_id, text, spoiler_text, sensitive,
  ordered_media_attachment_ids, media_descriptions, poll_options, quote_id,
  created_at, updated_at
) VALUES (
  9401, 116844606259202001, 116845105643525105,
  'Edited remote status before the current revision', '', false,
  ARRAY[]::bigint[], ARRAY[]::text[], NULL, NULL,
  '2026-07-01 14:07:30', '2026-07-01 14:07:30'
);

INSERT INTO status_edits (
  id, account_id, status_id, text, spoiler_text, sensitive,
  ordered_media_attachment_ids, media_descriptions, poll_options, quote_id,
  created_at, updated_at
) VALUES (
  9402, 116844606259201001, 116846257766400501,
  'Deleted status edit must not be returned', '', false,
  NULL, NULL, NULL, NULL,
  '2026-07-01 19:15:00', '2026-07-01 19:15:00'
);

INSERT INTO status_edits (
  id, account_id, status_id, text, spoiler_text, sensitive,
  ordered_media_attachment_ids, media_descriptions, poll_options, quote_id,
  created_at, updated_at
) VALUES (
  9403, 116844606259201001, 116844842188805001,
  'Public status snapshot with a nullable media description', '', false,
  ARRAY[-101, 116844842188806001]::bigint[],
  ARRAY[NULL, 'Deterministic Mastodon test attachment']::text[], NULL, NULL,
  '2026-07-01 13:00:30', '2026-07-01 13:00:30'
);

INSERT INTO mentions (id, account_id, status_id, silent, created_at, updated_at) VALUES
  (7001, 116844606259201001, 116845078118405101, false, '2026-07-01 14:00:00', '2026-07-01 14:00:00'),
  (7002, 116844606259202001, 116844853985285004, false, '2026-07-01 13:03:00', '2026-07-01 13:03:00'),
  (7003, 116844606259202001, 116846257766400501, false, '2026-07-01 19:10:00', '2026-07-01 19:10:00');

INSERT INTO media_attachments (
  id, account_id, status_id, type, processing, description, remote_url,
  created_at, updated_at
) VALUES
  (
    116844842188806001, 116844606259201001, 116844842188805001, 0, 2,
    'Deterministic Mastodon test attachment', '',
    '2026-07-01 13:00:00', '2026-07-01 13:00:00'
  ),
  (-101, 116844606259201001, 116844842188805001, 0, 2, 'First ordered attachment', 'https://media.fixture.invalid/first.jpg', '2026-07-01 13:00:00', '2026-07-01 13:00:00'),
  (-102, 116844606259201001, 116844842188805001, 0, 2, 'Third ordered attachment', 'https://media.fixture.invalid/third.jpg', '2026-07-01 13:00:00', '2026-07-01 13:00:00'),
  (-103, 116844606259201001, 116844842188805001, 0, 2, 'Fourth ordered attachment', 'https://media.fixture.invalid/fourth.jpg', '2026-07-01 13:00:00', '2026-07-01 13:00:00'),
  (-104, 116844606259201001, 116844842188805001, 0, 2, 'Over-limit ordered attachment', 'https://media.fixture.invalid/fifth.jpg', '2026-07-01 13:00:00', '2026-07-01 13:00:00'),
  (-105, 116844606259201001, 116844842188805001, 0, 2, 'Stale unordered attachment', 'https://media.fixture.invalid/stale.jpg', '2026-07-01 13:00:00', '2026-07-01 13:00:00'),
  (-210, 116844606259201001, 116844846120965002, 0, 2, 'NULL-order fallback first', 'https://media.fixture.invalid/fallback-first.jpg', '2026-07-01 13:01:00', '2026-07-01 13:01:00'),
  (-209, 116844606259201001, 116844846120965002, 0, 2, 'NULL-order fallback second', 'https://media.fixture.invalid/fallback-second.jpg', '2026-07-01 13:01:00', '2026-07-01 13:01:00'),
  (-208, 116844606259201001, 116844846120965002, 0, 2, 'NULL-order fallback third', 'https://media.fixture.invalid/fallback-third.jpg', '2026-07-01 13:01:00', '2026-07-01 13:01:00'),
  (-207, 116844606259201001, 116844846120965002, 0, 2, 'NULL-order fallback fourth', 'https://media.fixture.invalid/fallback-fourth.jpg', '2026-07-01 13:01:00', '2026-07-01 13:01:00'),
  (-206, 116844606259201001, 116844846120965002, 0, 2, 'NULL-order fallback over limit', 'https://media.fixture.invalid/fallback-fifth.jpg', '2026-07-01 13:01:00', '2026-07-01 13:01:00');

INSERT INTO media_attachments (
  id, account_id, status_id, type, processing, description, remote_url,
  created_at, updated_at
) VALUES (
  -98, 116844606259201001, 116846257766400501, 0, 2,
  'Deleted status media must not be returned', '',
  '2026-07-01 19:10:00', '2026-07-01 19:10:00'
);

INSERT INTO polls (
  id, account_id, status_id, options, cached_tallies, votes_count,
  voters_count, multiple, hide_totals, expires_at, last_fetched_at,
  created_at, updated_at
) VALUES (
  8201, 116844606259202001, 111680579174405102, ARRAY['Tea', 'Coffee'], ARRAY[1, 0]::bigint[], 1,
  1, false, false, '2024-01-02 12:00:00', '2024-01-02 12:00:00',
  '2024-01-01 12:00:00', '2024-01-02 12:00:00'
);

INSERT INTO polls (
  id, account_id, status_id, options, cached_tallies, votes_count,
  voters_count, multiple, hide_totals, expires_at, created_at, updated_at
) VALUES (
  8203, 116844606259201001, 116846257766400501, ARRAY['Deleted'], ARRAY[1]::bigint[],
  1, 1, false, false, '2026-07-01 20:00:00',
  '2026-07-01 19:00:00', '2026-07-01 19:30:00'
);

UPDATE statuses SET poll_id = 8201 WHERE id = 111680579174405102;
UPDATE statuses SET poll_id = 8203 WHERE id = 116846257766400501;

INSERT INTO poll_votes (id, account_id, poll_id, choice, uri, created_at, updated_at) VALUES (
  8202, 116844606259201001, 8201, 0,
  'https://fixture-v4-6-5.rustodon.invalid/users/alice#votes/8202',
  '2024-01-01 13:00:00', '2024-01-01 13:00:00'
);

INSERT INTO poll_votes (id, account_id, poll_id, choice, uri, created_at, updated_at) VALUES (
  8204, 116844606259201001, 8203, 0, NULL,
  '2026-07-01 19:10:00', '2026-07-01 19:10:00'
);

INSERT INTO follows (
  id, account_id, target_account_id, show_reblogs, notify, languages, uri,
  created_at, updated_at
) VALUES
  (8001, 116844606259201001, 116844606259202001, true, true, ARRAY['en'], 'https://fixture-v4-6-5.rustodon.invalid/users/alice#follows/8001', '2026-07-01 12:10:00', '2026-07-01 12:10:00'),
  (8002, 116844606259202001, 116844606259201001, true, false, NULL, 'https://remote.fixture.invalid/users/bob#follows/8002', '2026-07-01 12:11:00', '2026-07-01 12:11:00');

INSERT INTO follow_requests (
  id, account_id, target_account_id, show_reblogs, notify, languages, uri,
  created_at, updated_at
) VALUES (
  8003, 116844606259202002, 116844606259201001, true, false, ARRAY['en'],
  'https://remote.fixture.invalid/users/carol#follows/8003',
  '2026-07-01 12:12:00', '2026-07-01 12:12:00'
);

INSERT INTO favourites (id, account_id, status_id, created_at, updated_at) VALUES
  (8101, 116844606259202001, 116844842188805001, '2026-07-01 15:03:00', '2026-07-01 15:03:00'),
  (8102, 116844606259202001, 116846257766400501, '2026-07-01 19:10:00', '2026-07-01 19:10:00');

INSERT INTO bookmarks (id, account_id, status_id, created_at, updated_at) VALUES
  (9501, 116844606259201001, 116845078118405101, '2026-07-01 15:04:00', '2026-07-01 15:04:00'),
  (9507, 116844606259201001, 116846257766400501, '2026-07-01 19:10:00', '2026-07-01 19:10:00');

INSERT INTO status_pins (id, account_id, status_id, created_at, updated_at) VALUES
  (9505, 116844606259201001, 116844842188805001, '2026-07-01 15:03:00', '2026-07-01 15:03:00'),
  (9508, 116844606259201001, 116846257766400501, '2026-07-01 19:10:00', '2026-07-01 19:10:00');

INSERT INTO blocks (
  id, account_id, target_account_id, uri, created_at, updated_at
) VALUES (
  9502, 116844606259201001, 116844606259202002,
  'https://fixture-v4-6-5.rustodon.invalid/users/alice#blocks/9502',
  '2026-07-01 15:05:00', '2026-07-01 15:05:00'
);

INSERT INTO mutes (
  id, account_id, target_account_id, hide_notifications, expires_at,
  created_at, updated_at
) VALUES (
  9503, 116844606259201001, 116844606259202001, false,
  '2026-08-01 00:00:00', '2026-07-01 15:06:00', '2026-07-01 15:06:00'
);

INSERT INTO account_domain_blocks (
  id, account_id, domain, created_at, updated_at
) VALUES (
  9504, 116844606259201001, 'account-blocked.fixture.invalid',
  '2026-07-01 15:07:00', '2026-07-01 15:07:00'
);

INSERT INTO lists (id, account_id, title, replies_policy, exclusive, created_at, updated_at) VALUES
  (9001, 116844606259201001, 'Fixture normal list', 0, false, '2026-07-01 12:20:00', '2026-07-01 12:20:00'),
  (9002, 116844606259201001, 'Fixture exclusive list', 1, true, '2026-07-01 12:21:00', '2026-07-01 12:21:00');

INSERT INTO list_accounts (id, list_id, account_id, follow_id) VALUES
  (9003, 9001, 116844606259202001, 8001),
  (9004, 9002, 116844606259202001, 8001);

INSERT INTO custom_filters (
  id, account_id, phrase, context, action, expires_at, created_at, updated_at
) VALUES (
  9101, 116844606259201001, 'Readable fixture filter', ARRAY['home', 'notifications'], 1,
  NULL, '2026-07-01 12:22:00', '2026-07-01 12:22:00'
);

INSERT INTO custom_filter_keywords (
  id, custom_filter_id, keyword, whole_word, created_at, updated_at
) VALUES (
  9102, 9101, 'spoiler fixture', true,
  '2026-07-01 12:22:00', '2026-07-01 12:22:00'
);

INSERT INTO custom_filter_statuses (
  id, custom_filter_id, status_id, created_at, updated_at
) VALUES (
  9103, 9101, 116845105643525105, '2026-07-01 12:22:00', '2026-07-01 12:22:00'
);

INSERT INTO custom_filter_statuses (
  id, custom_filter_id, status_id, created_at, updated_at
) VALUES (
  9104, 9101, 116846257766400501, '2026-07-01 19:10:00', '2026-07-01 19:10:00'
);

INSERT INTO domain_allows (id, domain, created_at, updated_at) VALUES (
  9601, 'allowed.fixture.invalid', '2026-07-01 15:10:00', '2026-07-01 15:10:00'
);

INSERT INTO domain_blocks (
  id, domain, severity, reject_media, reject_reports, private_comment,
  public_comment, obfuscate, created_at, updated_at
) VALUES (
  9602, 'blocked.fixture.invalid', 99, true, false,
  'Raw private moderation note', '', true,
  '2026-07-01 15:11:00', '2026-07-01 15:11:00'
);

INSERT INTO notification_policies (
  id, account_id, for_bots, for_limited_accounts, for_new_accounts,
  for_not_followers, for_not_following, for_private_mentions,
  created_at, updated_at
) VALUES (
  9701, 116844606259201001, 0, 1, 2, 0, 99, 1,
  '2026-07-01 15:12:00', '2026-07-01 15:12:00'
);

INSERT INTO notification_permissions (
  id, account_id, from_account_id, created_at, updated_at
) VALUES (
  9702, 116844606259201001, 116844606259202001,
  '2026-07-01 15:13:00', '2026-07-01 15:13:00'
);

INSERT INTO notification_requests (
  id, account_id, from_account_id, last_status_id, notifications_count,
  created_at, updated_at
) VALUES (
  116846261698560601, 116844606259201001, 116844606259202001,
  116845078118405101, 1, '2026-07-01 19:01:00', '2026-07-01 19:01:00'
);

INSERT INTO notification_requests (
  id, account_id, from_account_id, last_status_id, notifications_count,
  created_at, updated_at
) VALUES (
  -96, 116844606259201001, 116844606259202002,
  116846257766400501, 1, '2026-07-01 19:02:00', '2026-07-01 19:02:00'
);

INSERT INTO notification_requests (
  id, account_id, from_account_id, last_status_id, notifications_count,
  created_at, updated_at
) VALUES (
  -95, 116844606259201001, 116844606259202003,
  116844842188805001, 1, '2026-07-01 19:03:00', '2026-07-01 19:03:00'
);

INSERT INTO settings (id, var, value, created_at, updated_at) VALUES
  (9801, 'fixture_scalar', $yaml$--- true
$yaml$, '2026-07-01 15:14:00', '2026-07-01 15:14:00'),
  (9802, 'fixture_tagged', $yaml$--- !ruby/hash:ActiveSupport::HashWithIndifferentAccess
fixture: value
$yaml$, '2026-07-01 15:15:00', '2026-07-01 15:15:00');

INSERT INTO quotes (
  id, account_id, status_id, quoted_account_id, quoted_status_id, state,
  activity_uri, approval_uri, legacy, created_at, updated_at
) VALUES
  (116845317980168701, 116844606259202001, 116845317980165202, 116844606259201001, 116844842188805001, 1, 'https://remote.fixture.invalid/activities/quote-116845317980168701', NULL, false, '2026-07-01 15:01:00', '2026-07-01 15:01:00'),
  (116845314048008702, 116844606259201001, 116845314048005201, 116844606259202001, 116845093847045103, 1, 'https://fixture-v4-6-5.rustodon.invalid/users/alice/quote_requests/116845314048008702', 'https://remote.fixture.invalid/activities/accept-116845314048008702', false, '2026-07-01 15:00:00', '2026-07-01 15:00:00');

INSERT INTO quotes (
  id, account_id, status_id, quoted_account_id, quoted_status_id, state,
  activity_uri, approval_uri, legacy, created_at, updated_at
) VALUES (
  -97, 116844606259201001, 116846257766400501, 116844606259201001,
  116844842188805001, 0, NULL, NULL, false,
  '2026-07-01 19:10:00', '2026-07-01 19:10:00'
);

INSERT INTO quotes (
  id, account_id, status_id, quoted_account_id, quoted_status_id, state,
  activity_uri, approval_uri, legacy, created_at, updated_at
) VALUES (
  -94, 116844606259201001, 116844846120965002, 116844606259201001,
  116846257766400501, 4, NULL, NULL, false,
  '2026-07-01 19:20:00', '2026-07-01 19:20:00'
);

INSERT INTO collections (
  id, account_id, name, description, description_html, local, sensitive,
  discoverable, item_count, original_number_of_items, language, uri, url,
  created_at, updated_at
) VALUES (
  116845549977608801, 116844606259202001, 'Remote fixture collection', 'Bob features Alice',
  '<p>Bob features Alice</p>', false, false, true, 1, 1, 'en',
  'https://remote.fixture.invalid/users/bob/collections/116845549977608801',
  'https://remote.fixture.invalid/@bob/collections/116845549977608801',
  '2026-07-01 16:00:00', '2026-07-01 16:00:00'
);

INSERT INTO collection_items (
  id, collection_id, account_id, position, state, activity_uri, approval_uri,
  object_uri, uri, approval_last_verified_at, created_at, updated_at
) VALUES (
  116845549977608802, 116845549977608801, 116844606259201001, 1, 1,
  'https://remote.fixture.invalid/activities/add-116845549977608802', NULL,
  'https://fixture-v4-6-5.rustodon.invalid/users/alice', NULL,
  '2026-07-01 16:01:00', '2026-07-01 16:00:00', '2026-07-01 16:01:00'
);

INSERT INTO keypairs (
  id, account_id, type, uri, public_key, private_key, revoked, expires_at,
  created_at, updated_at
) VALUES (
  8901, 116844606259202001, 0, 'https://remote.fixture.invalid/users/bob#secondary-key',
  $fixture_public$-----BEGIN PUBLIC KEY-----
MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEAqIAYvNFGbZ5g4iiK6feS
dXD4bDStFM58A7tHycYXaYtzZQpIeHXAmaXuZzXIwtrP4N0gIk8JNwZvXj2UPS+S
07t0V9wNK94he01LV5EMz/GN4eNnFmDL64HIEuKLvV8TvgjbUPRD6Y5X0UpKi2ZI
FLSb96Q5w0Z/k7ntpVKV52y8kz5Fjr/O/0JuHryZe0yItzJh8kzFfeMf0EXzfSna
KvT7P9jhgC6uTre+jXyvVZjiHDrnqvvucdI3I7DRfXo1OqARBrLjy+TdseUAjNYJ
+OuPRI1URIWQI01DCHqcohVu9+Ar+BiCjFp3ua+XMuJvrvbD61d1Fvig/9nbBRR+
8QIDAQAB
-----END PUBLIC KEY-----
$fixture_public$,
  NULL, false, NULL, '2026-07-01 16:10:00', '2026-07-01 16:10:00'
);

INSERT INTO keypairs (
  id, account_id, type, uri, public_key, private_key, revoked, expires_at,
  created_at, updated_at
) VALUES (
  8902, 116844606259201001, 0,
  'https://fixture-v4-6-5.rustodon.invalid/users/alice#opaque-key',
  'opaque-public-key-material',
  '{"p":"9q4gHslWnbK8a5zNjL1ySdpX5I8wunpM6ed7whTvAnwlyUs=","h":{"iv":"jvVW4mOA+IzCBTv/","at":"Wd+qB8DnQnN2bRo/7ukJmQ=="}}',
  false, NULL, '2026-07-01 16:11:00', '2026-07-01 16:11:00'
);

INSERT INTO tombstones (
  id, account_id, uri, by_moderator, created_at, updated_at
) VALUES (
  9901, 116844606259202001,
  'https://remote.fixture.invalid/users/bob/statuses/deleted-fixture', true,
  '2026-07-01 16:12:00', '2026-07-01 16:12:00'
);

INSERT INTO relationship_severance_events (
  id, type, target_name, purged, created_at, updated_at
) VALUES (
  8301, 0, 'blocked.fixture.invalid', false,
  '2026-07-01 17:00:00', '2026-07-01 17:00:00'
);

INSERT INTO severed_relationships (
  id, relationship_severance_event_id, local_account_id, remote_account_id,
  direction, show_reblogs, notify, languages, created_at, updated_at
) VALUES (
  8303, 8301, 116844606259201001, 116844606259202002, 0, true, false, ARRAY['en'],
  '2026-07-01 17:00:00', '2026-07-01 17:00:00'
);

INSERT INTO account_relationship_severance_events (
  id, account_id, relationship_severance_event_id, followers_count,
  following_count, created_at, updated_at
) VALUES (
  8302, 116844606259201001, 8301, 0, 1,
  '2026-07-01 17:00:00', '2026-07-01 17:00:00'
);

INSERT INTO reports (
  id, account_id, target_account_id, comment, category, status_ids,
  forwarded, uri, created_at, updated_at
) VALUES (
  8601, 116844606259201001, 116844606259202001, 'Readable fixture report', 1000, ARRAY[116845105643525105]::bigint[],
  false, 'https://fixture-v4-6-5.rustodon.invalid/users/alice/reports/8601',
  '2026-07-01 17:10:00', '2026-07-01 17:10:00'
);

INSERT INTO account_warnings (
  id, account_id, target_account_id, report_id, action, text, status_ids,
  created_at, updated_at
) VALUES (
  8401, 116844606259201002, 116844606259201001, NULL, 0, 'Readable fixture moderation warning',
  ARRAY['116844842188805001'], '2026-07-01 17:20:00', '2026-07-01 17:20:00'
);

INSERT INTO generated_annual_reports (
  id, account_id, year, schema_version, data, share_key, created_at, updated_at
) VALUES (
  8501, 116844606259201001, 2025, 2,
  '{"top_statuses":{"most_reblogged":116844842188805001},"most_reblogged_accounts":[],"commonly_interacted_with_accounts":[]}',
  'fixture-annual-report-share-key',
  '2026-07-01 17:30:00', '2026-07-01 17:30:00'
);

INSERT INTO notifications (
  id, account_id, activity_id, activity_type, from_account_id, type,
  group_key, filtered, created_at, updated_at
) VALUES
  (10001, 116844606259201001, 7001, 'Mention', 116844606259202001, 'mention', NULL, false, '2026-07-01 18:00:01', '2026-07-01 18:00:01'),
  (10002, 116844606259201001, 116845101711365104, 'Status', 116844606259202001, 'status', NULL, false, '2026-07-01 18:00:02', '2026-07-01 18:00:02'),
  (10003, 116844606259201001, 116845321912325301, 'Status', 116844606259202001, 'reblog', 'reblog-116844842188805001-495255', false, '2026-07-01 18:00:03', '2026-07-01 18:00:03'),
  (10004, 116844606259201001, 8002, 'Follow', 116844606259202001, 'follow', 'follow-495252', false, '2026-07-01 18:00:04', '2026-07-01 18:00:04'),
  (10005, 116844606259201001, 8003, 'FollowRequest', 116844606259202002, 'follow_request', NULL, false, '2026-07-01 18:00:05', '2026-07-01 18:00:05'),
  (10006, 116844606259201001, 8101, 'Favourite', 116844606259202001, 'favourite', 'favourite-116844842188805001-495255', false, '2026-07-01 18:00:06', '2026-07-01 18:00:06'),
  (10007, 116844606259201001, 8201, 'Poll', 116844606259202001, 'poll', NULL, false, '2026-07-01 18:00:07', '2026-07-01 18:00:07'),
  (10008, 116844606259201001, 116845105643525105, 'Status', 116844606259202001, 'update', NULL, false, '2026-07-01 18:00:08', '2026-07-01 18:00:08'),
  (10009, 116844606259201001, 8302, 'AccountRelationshipSeveranceEvent', 116844606259201001, 'severed_relationships', NULL, false, '2026-07-01 18:00:09', '2026-07-01 18:00:09'),
  (10010, 116844606259201001, 8401, 'AccountWarning', 116844606259201001, 'moderation_warning', NULL, false, '2026-07-01 18:00:10', '2026-07-01 18:00:10'),
  (10011, 116844606259201001, 8501, 'GeneratedAnnualReport', 116844606259201001, 'annual_report', NULL, false, '2026-07-01 18:00:11', '2026-07-01 18:00:11'),
  (10012, 116844606259201002, 116844606259201003, 'Account', 116844606259201003, 'admin.sign_up', 'admin.sign_up-495252', false, '2026-07-01 18:00:12', '2026-07-01 18:00:12'),
  (10013, 116844606259201002, 8601, 'Report', 116844606259201001, 'admin.report', NULL, false, '2026-07-01 18:00:13', '2026-07-01 18:00:13'),
  (10014, 116844606259201001, 116845317980168701, 'Quote', 116844606259202001, 'quote', NULL, false, '2026-07-01 18:00:14', '2026-07-01 18:00:14'),
  (10015, 116844606259201001, 116845314048005201, 'Status', 116844606259202001, 'quoted_update', NULL, false, '2026-07-01 18:00:15', '2026-07-01 18:00:15'),
  (10016, 116844606259201001, 116845549977608802, 'CollectionItem', 116844606259202001, 'added_to_collection', NULL, false, '2026-07-01 18:00:16', '2026-07-01 18:00:16'),
  (10017, 116844606259201001, 116845549977608801, 'Collection', 116844606259202001, 'collection_update', NULL, false, '2026-07-01 18:00:17', '2026-07-01 18:00:17'),
  (10018, 116844606259201001, 116846257766400501, 'FutureActivity', 116844606259202001, 'future_event', NULL, true, '2026-07-01 18:00:18', '2026-07-01 18:00:18'),
  (10019, 116844606259201001, 116846257766400501, 'FutureActivity', 116844606259202001, NULL, NULL, true, '2026-07-01 18:00:19', '2026-07-01 18:00:19'),
  (10020, 116844606259201001, 116846257766400501, 'Status', 116844606259202001, 'future_deleted_status', NULL, true, '2026-07-01 18:00:20', '2026-07-01 18:00:20');

INSERT INTO notifications (
  id, account_id, activity_id, activity_type, from_account_id, type,
  group_key, filtered, created_at, updated_at
) VALUES (
  10021, 116844606259201001, 116844606259202003, 'FutureActivity',
  116844606259202003, 'future_suspended', NULL, true,
  '2026-07-01 18:00:21', '2026-07-01 18:00:21'
);

INSERT INTO account_stats (
  id, account_id, statuses_count, following_count, followers_count,
  last_status_at, created_at, updated_at
) VALUES
  (11001, 116844606259201001, 5, 1, 1, '2026-07-01', '2026-07-01 18:10:00', '2026-07-01 18:10:00'),
  (11002, 116844606259201002, 0, 0, 0, NULL, '2026-07-01 18:10:00', '2026-07-01 18:10:00'),
  (11003, 116844606259201003, 0, 0, 0, NULL, '2026-07-01 18:10:00', '2026-07-01 18:10:00'),
  (11004, 116844606259202001, 7, 1, 1, '2026-07-01', '2026-07-01 18:10:00', '2026-07-01 18:10:00'),
  (11005, 116844606259202002, 0, 0, 0, NULL, '2026-07-01 18:10:00', '2026-07-01 18:10:00'),
  (11006, -99, 0, 0, 0, NULL, '2026-07-01 18:10:00', '2026-07-01 18:10:00'),
  (11007, 116844606259202003, 0, 0, 0, NULL, '2026-07-01 18:10:00', '2026-07-01 18:10:00');

INSERT INTO status_stats (
  id, status_id, replies_count, reblogs_count, favourites_count, quotes_count,
  created_at, updated_at
) SELECT
  CASE id
    WHEN 116844842188805001 THEN 12001
    WHEN 116844846120965002 THEN 12002
    WHEN 116844850053125003 THEN 12003
    WHEN 116844853985285004 THEN 12004
    WHEN 116844857917445005 THEN 12005
    WHEN 116845078118405101 THEN 12006
    WHEN 111680579174405102 THEN 12007
    WHEN 116845093847045103 THEN 12008
    WHEN 116845101711365104 THEN 12009
    WHEN 116845105643525105 THEN 12010
    WHEN 116845314048005201 THEN 12011
    WHEN 116845317980165202 THEN 12012
    WHEN 116845321912325301 THEN 12013
    WHEN 116846257766400501 THEN 12014
  END,
  id,
  0,
  CASE WHEN id = 116844842188805001 THEN 1 ELSE 0 END,
  CASE WHEN id = 116844842188805001 THEN 1 ELSE 0 END,
  CASE WHEN id IN (116844842188805001, 116845093847045103) THEN 1 ELSE 0 END,
  '2026-07-01 18:11:00',
  '2026-07-01 18:11:00'
FROM statuses;

-- Set every serial sequence from table contents and separately set the seven
-- Snowflake backing sequences whose column default is timestamp_id().
DO $$
DECLARE
  item record;
  sequence_name text;
  maximum_id bigint;
BEGIN
  FOR item IN
    SELECT table_name
    FROM information_schema.columns
    WHERE table_schema = 'public' AND column_name = 'id'
    ORDER BY table_name
  LOOP
    sequence_name := pg_get_serial_sequence(format('public.%I', item.table_name), 'id');
    IF sequence_name IS NOT NULL THEN
      EXECUTE format('SELECT max(id) FROM public.%I', item.table_name) INTO maximum_id;
      IF maximum_id IS NULL THEN
        PERFORM setval(sequence_name, 1, false);
      ELSE
        PERFORM setval(sequence_name, maximum_id, true);
      END IF;
    END IF;
  END LOOP;
END
$$;

SELECT setval('accounts_id_seq', 6, true);
SELECT setval('statuses_id_seq', 14, true);
SELECT setval('media_attachments_id_seq', 1, true);
SELECT setval('quotes_id_seq', 2, true);
SELECT setval('collections_id_seq', 1, true);
SELECT setval('collection_items_id_seq', 1, true);
SELECT setval('notification_requests_id_seq', 1, true);

REFRESH MATERIALIZED VIEW account_summaries;
REFRESH MATERIALIZED VIEW global_follow_recommendations;
REFRESH MATERIALIZED VIEW instances;

COMMIT;
