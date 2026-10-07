-- lmu-relay schema v1 (D-007): accounts, refresh tokens and session metadata.
-- Never telemetry samples. Connection codes and engineer resume tickets are never stored here
-- (relay memory only). Refresh tokens and driver resume tokens are stored as SHA-256 digests only.

CREATE TABLE accounts (
    id              UUID        PRIMARY KEY,
    -- normalized (lower-case) login name
    username        TEXT        NOT NULL UNIQUE CHECK (username = lower(username)),
    display_name    TEXT        NOT NULL,
    role            TEXT        NOT NULL CHECK (role IN ('driver', 'engineer', 'admin')),
    -- Argon2id PHC string
    password_hash   TEXT        NOT NULL,
    disabled        BOOLEAN     NOT NULL DEFAULT FALSE,
    created_at      TIMESTAMPTZ NOT NULL,
    last_login_at   TIMESTAMPTZ
);

-- One family per login; revoking a family logs that device out.
CREATE TABLE token_families (
    id          UUID        PRIMARY KEY,
    account_id  UUID        NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    created_at  TIMESTAMPTZ NOT NULL,
    revoked_at  TIMESTAMPTZ
);
CREATE INDEX token_families_account_idx ON token_families (account_id);

CREATE TABLE refresh_tokens (
    id          UUID        PRIMARY KEY,
    family_id   UUID        NOT NULL REFERENCES token_families (id) ON DELETE CASCADE,
    account_id  UUID        NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    token_hash  BYTEA       NOT NULL UNIQUE CHECK (octet_length(token_hash) = 32),
    created_at  TIMESTAMPTZ NOT NULL,
    -- sliding lifetime: every use rotates the token and the successor gets a fresh expiry
    expires_at  TIMESTAMPTZ NOT NULL,
    rotated_at  TIMESTAMPTZ,
    revoked_at  TIMESTAMPTZ
);
CREATE INDEX refresh_tokens_family_idx ON refresh_tokens (family_id);
CREATE INDEX refresh_tokens_account_idx ON refresh_tokens (account_id);
CREATE INDEX refresh_tokens_expiry_idx ON refresh_tokens (expires_at);

CREATE TABLE sessions (
    id                  BIGINT      PRIMARY KEY CHECK (id > 0),
    driver_account      UUID        NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    -- SHA-256 of the driver's resume token; NULL once the session is closed
    resume_hash         BYTEA       CHECK (resume_hash IS NULL OR octet_length(resume_hash) = 32),
    car                 TEXT,
    vehicle_class       TEXT,
    track               TEXT,
    -- raw LMU mSession value
    session_type        INTEGER,
    lmu_version         TEXT,
    app_version         TEXT        NOT NULL,
    schema_version      INTEGER     NOT NULL CHECK (schema_version BETWEEN 0 AND 65535),
    started_at          TIMESTAMPTZ NOT NULL,
    status              TEXT        NOT NULL CHECK (status IN ('live', 'grace', 'closed')),
    close_reason        TEXT,
    last_activity       TIMESTAMPTZ NOT NULL,
    ended_at            TIMESTAMPTZ
);
CREATE INDEX sessions_open_idx ON sessions (status, last_activity) WHERE status <> 'closed';
CREATE INDEX sessions_driver_idx ON sessions (driver_account);
