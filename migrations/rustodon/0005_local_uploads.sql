-- No FK to public tables: deletion must not discard the last cleanup manifest.
CREATE TABLE rustodon.local_uploads (
    media_id bigint NOT NULL,
    account_id bigint NOT NULL,
    generation bigint NOT NULL,
    accepted boolean NOT NULL DEFAULT false,
    claim bigint NOT NULL DEFAULT 0,
    raw_mime text COLLATE "C" NOT NULL,
    raw_size bigint NOT NULL,
    raw_sha256 bytea NOT NULL,
    raw_path text COLLATE "C" NOT NULL,
    output_paths text[] COLLATE "C" NOT NULL DEFAULT '{}',
    created_at timestamp with time zone NOT NULL DEFAULT clock_timestamp(),
    CONSTRAINT local_uploads_pkey PRIMARY KEY (media_id),
    CONSTRAINT local_uploads_identity_check CHECK (
        media_id > 0 AND account_id > 0 AND generation > 0 AND claim >= 0
    ),
    CONSTRAINT local_uploads_claim_check CHECK (accepted OR claim = 0),
    CONSTRAINT local_uploads_mime_check CHECK (octet_length(raw_mime) BETWEEN 1 AND 255),
    CONSTRAINT local_uploads_size_check CHECK (raw_size BETWEEN 1 AND 1073741824),
    CONSTRAINT local_uploads_hash_check CHECK (octet_length(raw_sha256) = 32),
    CONSTRAINT local_uploads_raw_path_check CHECK (
        raw_path = 'local_uploads/' || media_id::text || '/' || generation::text || '/input'
    ),
    CONSTRAINT local_uploads_outputs_check CHECK (
        cardinality(output_paths) <= 2 AND array_position(output_paths, NULL) IS NULL
        AND (cardinality(output_paths) = 0 OR (accepted AND claim > 0))
        AND octet_length(output_paths::text) <= 2048
    )
);
REVOKE ALL ON TABLE rustodon.local_uploads FROM PUBLIC;
