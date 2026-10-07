-- Engineer slots of open sessions (D-022): lets a running engineer app resume with its in-memory
-- ticket after a relay restart. Session metadata only: no ticket, ticket digest or connection code
-- is stored here (tickets carry an HMAC tag, see crates/relay/src/tickets.rs). A row exists exactly
-- while the slot does; deleting it (DISCONNECT, ENGINEER_KICK, ticket expiry, session end) kills the
-- slot's tickets.

CREATE TABLE session_engineers (
    session_id      BIGINT      NOT NULL REFERENCES sessions (id) ON DELETE CASCADE,
    account_id      UUID        NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    -- relay-assigned connection id (ENGINEER_LIST / ENGINEER_KICK), unique within the session
    connection_id   BIGINT      NOT NULL CHECK (connection_id > 0),
    connected_since TIMESTAMPTZ NOT NULL,
    -- when the engineer's connection dropped (start of the ticket window); NULL while connected
    dropped_at      TIMESTAMPTZ,
    -- one slot per engineer account and session (a second join takes the slot over)
    PRIMARY KEY (session_id, account_id)
);
