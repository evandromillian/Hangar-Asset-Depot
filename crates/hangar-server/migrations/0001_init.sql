-- Hangar initial schema.

CREATE TABLE users (
    id            UUID PRIMARY KEY,
    username      TEXT NOT NULL UNIQUE,
    display_name  TEXT NOT NULL,
    password_hash TEXT NOT NULL,
    is_admin      BOOLEAN NOT NULL DEFAULT FALSE,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE projects (
    id          UUID PRIMARY KEY,
    slug        TEXT NOT NULL UNIQUE,
    name        TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    created_by  UUID REFERENCES users(id) ON DELETE SET NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE project_members (
    project_id UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    user_id    UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    role       TEXT NOT NULL CHECK (role IN ('owner', 'editor', 'viewer')),
    added_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (project_id, user_id)
);
CREATE INDEX project_members_user_idx ON project_members(user_id);

-- Content-addressed store: one row per unique file content (BLAKE3).
CREATE TABLE blobs (
    hash       TEXT PRIMARY KEY CHECK (hash ~ '^[0-9a-f]{64}$'),
    size       BIGINT NOT NULL,
    mime       TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE assets (
    id              UUID PRIMARY KEY,
    project_id      UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    slug            TEXT NOT NULL,
    name            TEXT NOT NULL,
    kind            TEXT NOT NULL,
    description     TEXT NOT NULL DEFAULT '',
    tags            JSONB NOT NULL DEFAULT '[]'::jsonb,
    source          JSONB NOT NULL DEFAULT '{}'::jsonb,
    head_version_id UUID,
    created_by      UUID REFERENCES users(id) ON DELETE SET NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (project_id, slug)
);
CREATE INDEX assets_project_idx ON assets(project_id, updated_at DESC);
CREATE INDEX assets_tags_idx ON assets USING GIN (tags);

-- Immutable snapshots of an asset. parent_id forms a DAG (branches allowed).
CREATE TABLE asset_versions (
    id           UUID PRIMARY KEY,
    asset_id     UUID NOT NULL REFERENCES assets(id) ON DELETE CASCADE,
    number       INTEGER NOT NULL,
    parent_id    UUID REFERENCES asset_versions(id) ON DELETE SET NULL,
    message      TEXT NOT NULL DEFAULT '',
    author_id    UUID REFERENCES users(id) ON DELETE SET NULL,
    status       TEXT NOT NULL DEFAULT 'pending',
    error        TEXT,
    preview      JSONB,
    file_count   INTEGER NOT NULL DEFAULT 0,
    total_size   BIGINT NOT NULL DEFAULT 0,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    processed_at TIMESTAMPTZ,
    UNIQUE (asset_id, number)
);

ALTER TABLE assets
    ADD CONSTRAINT assets_head_fk FOREIGN KEY (head_version_id)
    REFERENCES asset_versions(id) ON DELETE SET NULL;

-- The file tree of each version.
CREATE TABLE version_files (
    version_id UUID NOT NULL REFERENCES asset_versions(id) ON DELETE CASCADE,
    path       TEXT NOT NULL,
    blob_hash  TEXT NOT NULL REFERENCES blobs(hash),
    size       BIGINT NOT NULL,
    PRIMARY KEY (version_id, path)
);
CREATE INDEX version_files_blob_idx ON version_files(blob_hash);

-- Derived artifacts produced by the pipeline (LOD GLBs, thumbnails, previews).
CREATE TABLE renditions (
    version_id UUID NOT NULL REFERENCES asset_versions(id) ON DELETE CASCADE,
    kind       TEXT NOT NULL,
    key        TEXT NOT NULL DEFAULT '',
    blob_hash  TEXT NOT NULL REFERENCES blobs(hash),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (version_id, kind, key)
);
CREATE INDEX renditions_blob_idx ON renditions(blob_hash);

-- Durable job queue (claimed with FOR UPDATE SKIP LOCKED).
CREATE TABLE jobs (
    id           BIGSERIAL PRIMARY KEY,
    kind         TEXT NOT NULL,
    payload      JSONB NOT NULL DEFAULT '{}'::jsonb,
    status       TEXT NOT NULL DEFAULT 'queued' CHECK (status IN ('queued', 'running', 'done', 'failed')),
    attempts     INTEGER NOT NULL DEFAULT 0,
    max_attempts INTEGER NOT NULL DEFAULT 3,
    run_after    TIMESTAMPTZ NOT NULL DEFAULT now(),
    locked_at    TIMESTAMPTZ,
    last_error   TEXT,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    finished_at  TIMESTAMPTZ
);
CREATE INDEX jobs_queue_idx ON jobs(run_after, id) WHERE status = 'queued';
