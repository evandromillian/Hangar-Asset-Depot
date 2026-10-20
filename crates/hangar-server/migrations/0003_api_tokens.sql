-- Long-lived access tokens for editor plugins and the CLI. Only the BLAKE3
-- hash of a token is stored; the token itself is shown once.
CREATE TABLE api_tokens (
    id           UUID PRIMARY KEY,
    user_id      UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name         TEXT NOT NULL,
    client       TEXT NOT NULL,
    token_hash   TEXT NOT NULL UNIQUE,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_used_at TIMESTAMPTZ
);
CREATE INDEX api_tokens_user ON api_tokens(user_id);

-- Device pairing (OAuth device-flow style): a plugin starts one, the user
-- approves it on the web, the plugin's next poll creates the token.
CREATE TABLE device_pairings (
    id               UUID PRIMARY KEY,
    device_code_hash TEXT NOT NULL UNIQUE,
    user_code        TEXT NOT NULL UNIQUE,
    client           TEXT NOT NULL,
    device_name      TEXT NOT NULL,
    detail           TEXT,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at       TIMESTAMPTZ NOT NULL,
    -- 'pending', 'approved', 'denied'; the row is deleted once the token is issued.
    status           TEXT NOT NULL DEFAULT 'pending',
    approved_by      UUID REFERENCES users(id) ON DELETE CASCADE
);
