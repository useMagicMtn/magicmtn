-- Serving resources use the same at-most-once dispatch receipts as sandboxes.
ALTER TABLE provider_resource_receipts RENAME TO old_provider_resource_receipts;
CREATE TABLE provider_resource_receipts (
    scope TEXT NOT NULL,
    kind TEXT NOT NULL CHECK(kind IN ('sandbox','inference')),
    binding TEXT NOT NULL CHECK(json_valid(binding)),
    state TEXT NOT NULL DEFAULT 'pending' CHECK(state IN ('pending','completed','uncertain')),
    result TEXT CHECK(result IS NULL OR json_valid(result)),
    PRIMARY KEY(scope,kind)
);
INSERT INTO provider_resource_receipts SELECT * FROM old_provider_resource_receipts;
DROP TABLE old_provider_resource_receipts;
CREATE TABLE local_models (
    id TEXT PRIMARY KEY,
    binding TEXT NOT NULL CHECK(json_valid(binding)),
    model TEXT NOT NULL CHECK(json_valid(model)),
    deployment TEXT CHECK(deployment IS NULL OR json_valid(deployment)),
    state TEXT NOT NULL DEFAULT 'selected'
);
CREATE TABLE local_pi_defaults (singleton INTEGER PRIMARY KEY CHECK(singleton=1), model_id TEXT NOT NULL REFERENCES local_models(id));
CREATE TABLE local_pi_sessions (
    id TEXT PRIMARY KEY,
    model_id TEXT NOT NULL REFERENCES local_models(id),
    provider TEXT NOT NULL,
    desired TEXT NOT NULL DEFAULT 'running' CHECK(desired IN ('running','stopped','terminated')),
    generation INTEGER NOT NULL DEFAULT 1,
    initialized INTEGER NOT NULL DEFAULT 0,
    snapshot TEXT NOT NULL DEFAULT '{}' CHECK(json_valid(snapshot)),
    created_at INTEGER NOT NULL DEFAULT (unixepoch())
);
CREATE TABLE local_pi_inputs (
    session_id TEXT NOT NULL REFERENCES local_pi_sessions(id),
    client_id TEXT NOT NULL,
    seq INTEGER NOT NULL,
    body TEXT NOT NULL CHECK(json_valid(body)),
    PRIMARY KEY(session_id,client_id,seq)
);
CREATE TABLE local_pi_inference (
    session_id TEXT NOT NULL REFERENCES local_pi_sessions(id),
    request_id TEXT NOT NULL,
    response TEXT CHECK(response IS NULL OR json_valid(response)),
    PRIMARY KEY(session_id,request_id)
);
CREATE TABLE local_pi_clients (session_id TEXT PRIMARY KEY REFERENCES local_pi_sessions(id), client_id TEXT NOT NULL, last_seq INTEGER NOT NULL DEFAULT -1, cols INTEGER NOT NULL DEFAULT 0, rows INTEGER NOT NULL DEFAULT 0);
CREATE TABLE local_pi_output (session_id TEXT NOT NULL REFERENCES local_pi_sessions(id), start INTEGER NOT NULL, data BLOB NOT NULL, PRIMARY KEY(session_id,start));
