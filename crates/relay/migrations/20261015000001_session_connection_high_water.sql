-- Per-session engineer connection-id high-water mark (D-022 clarification): the highest
-- connection id ever stored for the session. A session rebuilt after a relay restart continues
-- above it, so the id of a slot that ended before the restart is never handed out again (its
-- tickets, verified by tag over (session, account, connection id), stay dead, and a stale
-- ENGINEER_KICK cannot hit a different engineer). Raised together with each slot write.

ALTER TABLE sessions
    ADD COLUMN max_connection_id BIGINT NOT NULL DEFAULT 0 CHECK (max_connection_id >= 0);

-- Existing open sessions: cover the slots stored so far.
UPDATE sessions s
   SET max_connection_id = e.max_id
  FROM (SELECT session_id, MAX(connection_id) AS max_id
          FROM session_engineers GROUP BY session_id) e
 WHERE e.session_id = s.id;
