CREATE TABLE rustodon.durable_jobs (
    id bigint GENERATED ALWAYS AS IDENTITY,
    lane text COLLATE "C" NOT NULL,
    kind text COLLATE "C" NOT NULL,
    arguments jsonb NOT NULL,
    logical_key text COLLATE "C",
    run_at timestamp with time zone NOT NULL DEFAULT clock_timestamp(),
    attempts integer NOT NULL DEFAULT 0,
    max_attempts integer NOT NULL DEFAULT 25,
    lease_generation bigint NOT NULL DEFAULT 0,
    lease_owner text COLLATE "C",
    lease_expires_at timestamp with time zone,
    last_error text,
    dead_at timestamp with time zone,
    created_at timestamp with time zone NOT NULL DEFAULT clock_timestamp(),
    updated_at timestamp with time zone NOT NULL DEFAULT clock_timestamp(),
    CONSTRAINT durable_jobs_pkey PRIMARY KEY (id),
    CONSTRAINT durable_jobs_lane_check CHECK (
        lane IN ('ingress', 'core', 'push', 'pull', 'mail', 'maintenance')
    ),
    CONSTRAINT durable_jobs_kind_check CHECK (octet_length(kind) BETWEEN 1 AND 128),
    CONSTRAINT durable_jobs_logical_key_check CHECK (
        logical_key IS NULL OR octet_length(logical_key) BETWEEN 1 AND 1024
    ),
    CONSTRAINT durable_jobs_attempts_check CHECK (
        attempts >= 0 AND max_attempts > 0 AND attempts <= max_attempts
    ),
    CONSTRAINT durable_jobs_lease_generation_check CHECK (lease_generation >= 0),
    CONSTRAINT durable_jobs_lease_pair_check CHECK (
        (lease_owner IS NULL) = (lease_expires_at IS NULL)
    ),
    CONSTRAINT durable_jobs_dead_check CHECK (
        dead_at IS NULL OR (lease_owner IS NULL AND lease_expires_at IS NULL)
    )
);

CREATE INDEX durable_jobs_claim_idx
    ON rustodon.durable_jobs (lane, run_at, id)
    WHERE dead_at IS NULL;
CREATE INDEX durable_jobs_lease_idx
    ON rustodon.durable_jobs (lease_expires_at, id)
    WHERE lease_expires_at IS NOT NULL AND dead_at IS NULL;
CREATE INDEX durable_jobs_dead_idx
    ON rustodon.durable_jobs (dead_at DESC, id DESC)
    WHERE dead_at IS NOT NULL;
CREATE UNIQUE INDEX durable_jobs_logical_key_idx
    ON rustodon.durable_jobs (kind, logical_key)
    WHERE logical_key IS NOT NULL AND dead_at IS NULL;

CREATE TABLE rustodon.outbox_events (
    id bigint GENERATED ALWAYS AS IDENTITY,
    kind text COLLATE "C" NOT NULL,
    logical_key text COLLATE "C",
    payload jsonb NOT NULL,
    created_at timestamp with time zone NOT NULL DEFAULT clock_timestamp(),
    dispatched_at timestamp with time zone,
    CONSTRAINT outbox_events_pkey PRIMARY KEY (id),
    CONSTRAINT outbox_events_kind_check CHECK (octet_length(kind) BETWEEN 1 AND 128),
    CONSTRAINT outbox_events_logical_key_check CHECK (
        logical_key IS NULL OR octet_length(logical_key) BETWEEN 1 AND 1024
    ),
    CONSTRAINT outbox_events_dispatched_check CHECK (
        dispatched_at IS NULL OR dispatched_at >= created_at
    )
);

CREATE INDEX outbox_events_pending_idx
    ON rustodon.outbox_events (id)
    WHERE dispatched_at IS NULL;
CREATE UNIQUE INDEX outbox_events_logical_key_idx
    ON rustodon.outbox_events (kind, logical_key)
    WHERE logical_key IS NOT NULL;

CREATE TABLE rustodon.idempotency_keys (
    scope text COLLATE "C" NOT NULL,
    key text COLLATE "C" NOT NULL,
    fingerprint bytea NOT NULL,
    result jsonb NOT NULL,
    created_at timestamp with time zone NOT NULL DEFAULT clock_timestamp(),
    expires_at timestamp with time zone NOT NULL,
    CONSTRAINT idempotency_keys_pkey PRIMARY KEY (scope, key),
    CONSTRAINT idempotency_keys_scope_check CHECK (octet_length(scope) BETWEEN 1 AND 255),
    CONSTRAINT idempotency_keys_key_check CHECK (octet_length(key) BETWEEN 1 AND 255),
    CONSTRAINT idempotency_keys_fingerprint_check CHECK (octet_length(fingerprint) = 32),
    CONSTRAINT idempotency_keys_expiry_check CHECK (expires_at > created_at)
);

CREATE INDEX idempotency_keys_expires_idx ON rustodon.idempotency_keys (expires_at);

CREATE TABLE rustodon.ordering_markers (
    kind text COLLATE "C" NOT NULL,
    key_hash bytea NOT NULL,
    ordering_at timestamp with time zone NOT NULL,
    payload jsonb NOT NULL DEFAULT '{}'::jsonb,
    created_at timestamp with time zone NOT NULL DEFAULT clock_timestamp(),
    expires_at timestamp with time zone NOT NULL,
    CONSTRAINT ordering_markers_pkey PRIMARY KEY (kind, key_hash),
    CONSTRAINT ordering_markers_kind_check CHECK (octet_length(kind) BETWEEN 1 AND 128),
    CONSTRAINT ordering_markers_key_hash_check CHECK (octet_length(key_hash) = 32),
    CONSTRAINT ordering_markers_payload_check CHECK (jsonb_typeof(payload) = 'object'),
    CONSTRAINT ordering_markers_expiry_check CHECK (expires_at > created_at)
);

CREATE INDEX ordering_markers_expires_idx ON rustodon.ordering_markers (expires_at);

CREATE TABLE rustodon.domain_health (
    domain text COLLATE "C" NOT NULL,
    failures integer NOT NULL DEFAULT 0,
    last_failure_at timestamp with time zone,
    last_success_at timestamp with time zone,
    retry_at timestamp with time zone,
    last_error text,
    updated_at timestamp with time zone NOT NULL DEFAULT clock_timestamp(),
    CONSTRAINT domain_health_pkey PRIMARY KEY (domain),
    CONSTRAINT domain_health_domain_check CHECK (
        octet_length(domain) BETWEEN 1 AND 255
        AND domain = lower(domain)
        AND domain !~ '[[:space:]]'
    ),
    CONSTRAINT domain_health_failures_check CHECK (
        failures >= 0 AND (failures = 0 OR last_failure_at IS NOT NULL)
    ),
    CONSTRAINT domain_health_idle_check CHECK (
        failures > 0 OR (retry_at IS NULL AND last_error IS NULL)
    )
);

CREATE INDEX domain_health_retry_idx
    ON rustodon.domain_health (retry_at, domain)
    WHERE retry_at IS NOT NULL;

CREATE TABLE rustodon.heartbeats (
    process_id text COLLATE "C" NOT NULL,
    role text COLLATE "C" NOT NULL,
    lanes text[] NOT NULL DEFAULT ARRAY[]::text[],
    info jsonb NOT NULL DEFAULT '{}'::jsonb,
    started_at timestamp with time zone NOT NULL DEFAULT clock_timestamp(),
    heartbeat_at timestamp with time zone NOT NULL DEFAULT clock_timestamp(),
    CONSTRAINT heartbeats_pkey PRIMARY KEY (process_id),
    CONSTRAINT heartbeats_process_id_check CHECK (
        octet_length(process_id) BETWEEN 1 AND 255
    ),
    CONSTRAINT heartbeats_role_check CHECK (role IN ('worker', 'scheduler')),
    CONSTRAINT heartbeats_lanes_check CHECK (
        lanes <@ ARRAY['ingress', 'core', 'push', 'pull', 'mail', 'maintenance']::text[]
        AND (
            (role = 'worker' AND cardinality(lanes) > 0)
            OR (role = 'scheduler' AND cardinality(lanes) = 0)
        )
    ),
    CONSTRAINT heartbeats_info_check CHECK (jsonb_typeof(info) = 'object'),
    CONSTRAINT heartbeats_time_check CHECK (heartbeat_at >= started_at)
);

CREATE INDEX heartbeats_heartbeat_idx ON rustodon.heartbeats (heartbeat_at);

REVOKE ALL ON SCHEMA rustodon FROM PUBLIC;
REVOKE ALL ON ALL TABLES IN SCHEMA rustodon FROM PUBLIC;
REVOKE ALL ON ALL SEQUENCES IN SCHEMA rustodon FROM PUBLIC;
