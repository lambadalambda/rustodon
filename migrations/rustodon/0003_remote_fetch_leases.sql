CREATE TABLE rustodon.remote_fetch_leases (
    host text COLLATE "C" NOT NULL,
    lease_id text COLLATE "C" NOT NULL,
    expires_at timestamp with time zone NOT NULL,
    CONSTRAINT remote_fetch_leases_pkey PRIMARY KEY (host, lease_id),
    CONSTRAINT remote_fetch_leases_host_check CHECK (
        octet_length(host) BETWEEN 1 AND 255
    ),
    CONSTRAINT remote_fetch_leases_id_check CHECK (
        octet_length(lease_id) BETWEEN 1 AND 255
    ),
    CONSTRAINT remote_fetch_leases_expiry_check CHECK (
        expires_at > TIMESTAMP WITH TIME ZONE '1970-01-01 00:00:00+00'
    )
);

CREATE INDEX remote_fetch_leases_expires_idx
    ON rustodon.remote_fetch_leases (expires_at);

REVOKE ALL ON TABLE rustodon.remote_fetch_leases FROM PUBLIC;
