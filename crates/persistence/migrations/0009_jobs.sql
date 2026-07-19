CREATE TABLE jobs (
    job_id TEXT PRIMARY KEY NOT NULL,
    schema_version INTEGER NOT NULL CHECK (schema_version > 0),
    kind TEXT NOT NULL CHECK (length(kind) BETWEEN 1 AND 64),
    payload_schema_version INTEGER NOT NULL CHECK (payload_schema_version > 0),
    payload_json BLOB NOT NULL CHECK (length(payload_json) BETWEEN 1 AND 1048576),
    payload_sha256 TEXT NOT NULL CHECK (
        length(payload_sha256) = 64 AND payload_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    priority INTEGER NOT NULL,
    available_at_ms INTEGER NOT NULL CHECK (available_at_ms >= 0),
    max_attempts INTEGER NOT NULL CHECK (max_attempts BETWEEN 1 AND 100),
    attempt INTEGER NOT NULL DEFAULT 0 CHECK (attempt BETWEEN 0 AND max_attempts),
    retry_initial_ms INTEGER NOT NULL CHECK (retry_initial_ms > 0),
    retry_max_ms INTEGER NOT NULL CHECK (
        retry_max_ms >= retry_initial_ms AND retry_max_ms <= 604800000
    ),
    deduplication_key TEXT NOT NULL CHECK (length(deduplication_key) BETWEEN 1 AND 256),
    status TEXT NOT NULL CHECK (
        status IN (
            'queued', 'leased', 'succeeded', 'permanent_failure', 'exhausted',
            'cancelled'
        )
    ),
    owner TEXT CHECK (owner IS NULL OR length(owner) BETWEEN 1 AND 128),
    lease_generation INTEGER NOT NULL DEFAULT 0 CHECK (lease_generation >= 0),
    leased_at_ms INTEGER CHECK (leased_at_ms IS NULL OR leased_at_ms >= 0),
    lease_expires_at_ms INTEGER CHECK (
        lease_expires_at_ms IS NULL OR lease_expires_at_ms > leased_at_ms
    ),
    cancellation_requested_at_ms INTEGER CHECK (
        cancellation_requested_at_ms IS NULL OR cancellation_requested_at_ms >= 0
    ),
    last_error_code TEXT CHECK (last_error_code IS NULL OR length(last_error_code) BETWEEN 1 AND 64),
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
    updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= created_at_ms),
    finished_at_ms INTEGER CHECK (finished_at_ms IS NULL OR finished_at_ms >= created_at_ms),
    UNIQUE (kind, deduplication_key),
    CHECK (
        (status = 'leased' AND owner IS NOT NULL AND leased_at_ms IS NOT NULL
            AND lease_expires_at_ms IS NOT NULL AND finished_at_ms IS NULL)
        OR
        (status <> 'leased' AND owner IS NULL AND leased_at_ms IS NULL
            AND lease_expires_at_ms IS NULL)
    ),
    CHECK (status IN ('succeeded', 'permanent_failure', 'exhausted', 'cancelled')
        OR finished_at_ms IS NULL),
    CHECK (status NOT IN ('succeeded', 'permanent_failure', 'exhausted', 'cancelled')
        OR finished_at_ms IS NOT NULL),
    CHECK (leased_at_ms IS NULL OR leased_at_ms >= created_at_ms),
    CHECK (cancellation_requested_at_ms IS NULL
        OR cancellation_requested_at_ms >= created_at_ms)
) STRICT;

CREATE INDEX jobs_claim_idx
    ON jobs (status, kind, priority DESC, available_at_ms, created_at_ms, job_id);
CREATE INDEX jobs_lease_expiry_idx
    ON jobs (status, lease_expires_at_ms, job_id);

CREATE TABLE job_attempts (
    job_id TEXT NOT NULL,
    attempt INTEGER NOT NULL CHECK (attempt > 0),
    lease_generation INTEGER NOT NULL CHECK (lease_generation > 0),
    worker_id TEXT NOT NULL CHECK (length(worker_id) BETWEEN 1 AND 128),
    leased_at_ms INTEGER NOT NULL CHECK (leased_at_ms >= 0),
    lease_expires_at_ms INTEGER NOT NULL CHECK (lease_expires_at_ms > leased_at_ms),
    finished_at_ms INTEGER CHECK (finished_at_ms IS NULL OR finished_at_ms >= leased_at_ms),
    handler_outcome TEXT CHECK (
        handler_outcome IS NULL OR handler_outcome IN (
            'succeeded', 'retryable', 'permanent', 'cancelled', 'abandoned'
        )
    ),
    final_status TEXT CHECK (
        final_status IS NULL OR final_status IN (
            'queued', 'succeeded', 'permanent_failure', 'exhausted', 'cancelled'
        )
    ),
    error_code TEXT CHECK (error_code IS NULL OR length(error_code) BETWEEN 1 AND 64),
    next_available_at_ms INTEGER CHECK (
        next_available_at_ms IS NULL OR next_available_at_ms >= finished_at_ms
    ),
    PRIMARY KEY (job_id, attempt),
    UNIQUE (job_id, lease_generation),
    FOREIGN KEY (job_id) REFERENCES jobs (job_id) ON DELETE CASCADE,
    CHECK ((finished_at_ms IS NULL) = (handler_outcome IS NULL)),
    CHECK (handler_outcome <> 'retryable' OR error_code IS NOT NULL),
    CHECK (handler_outcome <> 'permanent' OR error_code IS NOT NULL)
) STRICT;

CREATE INDEX job_attempts_open_idx
    ON job_attempts (lease_expires_at_ms, job_id) WHERE finished_at_ms IS NULL;

UPDATE schema_metadata
SET value = '9', updated_at_ms = 0
WHERE key = 'application_schema_version';
