-- Human-readable path for a blob (local petname over the content hash).
--
-- The hash stays the canonical identifier and the primary key. `path` is a
-- display/lookup convenience: it is assigned locally (browser file picker,
-- later a rename box), never transported on the wire and never accepted from
-- a remote peer. It is therefore NOT unique — two blobs may share a path, and
-- a blob may have none.

ALTER TABLE blobs ADD COLUMN path TEXT;

CREATE INDEX IF NOT EXISTS idx_blobs_path ON blobs(path);
