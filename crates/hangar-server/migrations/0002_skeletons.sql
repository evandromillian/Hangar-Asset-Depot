-- Reference skeletons: system-wide meshes (e.g. UE4 Mannequin, UE5 Manny) that
-- animation-only versions are previewed on. The slug is what a version's
-- `.hangar/animation.json` refers to, so it never changes.
CREATE TABLE skeletons (
    id          UUID PRIMARY KEY,
    slug        TEXT NOT NULL UNIQUE,
    name        TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    asset_id    UUID NOT NULL UNIQUE REFERENCES assets(id) ON DELETE CASCADE,
    created_by  UUID REFERENCES users(id) ON DELETE SET NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
