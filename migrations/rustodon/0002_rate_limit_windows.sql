CREATE TABLE rustodon.rate_limit_windows (
    window_key text COLLATE "C" NOT NULL,
    bucket bigint NOT NULL,
    attempts integer NOT NULL,
    expires_at timestamp with time zone NOT NULL,
    CONSTRAINT rate_limit_windows_pkey PRIMARY KEY (window_key, bucket),
    CONSTRAINT rate_limit_windows_key_check CHECK (
        octet_length(window_key) BETWEEN 1 AND 255
    ),
    CONSTRAINT rate_limit_windows_bucket_check CHECK (bucket >= 0),
    CONSTRAINT rate_limit_windows_attempts_check CHECK (attempts > 0),
    CONSTRAINT rate_limit_windows_expiry_check CHECK (
        expires_at > TIMESTAMP WITH TIME ZONE '1970-01-01 00:00:00+00'
    )
);

CREATE INDEX rate_limit_windows_expires_idx
    ON rustodon.rate_limit_windows (expires_at);

REVOKE ALL ON TABLE rustodon.rate_limit_windows FROM PUBLIC;
