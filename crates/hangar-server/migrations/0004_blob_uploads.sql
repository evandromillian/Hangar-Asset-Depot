-- Who sent the bytes of each blob. Blobs are readable through project
-- membership (a version file or rendition of a project the user belongs to);
-- a commit may only reference a blob its author can read or uploaded, so
-- knowing a hash is not enough to pull a file into another project.
CREATE TABLE blob_uploads (
    blob_hash  TEXT NOT NULL REFERENCES blobs(hash) ON DELETE CASCADE,
    user_id    UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (blob_hash, user_id)
);
CREATE INDEX blob_uploads_user_idx ON blob_uploads(user_id);

-- Best effort for blobs stored before this table: the author of the first
-- version that referenced a blob is taken as its uploader.
INSERT INTO blob_uploads (blob_hash, user_id, created_at)
SELECT DISTINCT ON (f.blob_hash) f.blob_hash, v.author_id, v.created_at
FROM version_files f
JOIN asset_versions v ON v.id = f.version_id
WHERE v.author_id IS NOT NULL
ORDER BY f.blob_hash, v.created_at;
