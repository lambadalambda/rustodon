-- Exact daily unique users, with one sliding expiry per bucket, not per member.
-- No FK to public users/accounts: erasure or suspension must not rewrite history.
-- Both tables are maintained in one Rust transaction. Cleanup must delete members
-- and buckets together; no database cascade into or from the Mastodon schema.
CREATE TABLE rustodon.activity_buckets (
    day date NOT NULL,
    expires_at timestamp with time zone NOT NULL,
    CONSTRAINT activity_buckets_pkey PRIMARY KEY (day)
);
CREATE INDEX activity_buckets_expires_idx ON rustodon.activity_buckets (expires_at);
CREATE TABLE rustodon.activity_members (
    day date NOT NULL,
    user_id bigint NOT NULL,
    CONSTRAINT activity_members_pkey PRIMARY KEY (day, user_id)
);
REVOKE ALL ON TABLE rustodon.activity_buckets, rustodon.activity_members FROM PUBLIC;
