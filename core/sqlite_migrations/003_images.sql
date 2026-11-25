CREATE TABLE sandbox_images (
    name TEXT PRIMARY KEY,
    source TEXT NOT NULL,
    manifests TEXT NOT NULL CHECK(json_valid(manifests))
);
CREATE TABLE image_preparations (
    name TEXT NOT NULL REFERENCES sandbox_images(name),
    connection_id TEXT NOT NULL REFERENCES provider_connections(id),
    provider_sha256 TEXT NOT NULL,
    generation INTEGER NOT NULL,
    platform TEXT NOT NULL,
    repository TEXT NOT NULL,
    digest TEXT NOT NULL,
    result TEXT NOT NULL CHECK(json_valid(result)),
    PRIMARY KEY(name,connection_id,provider_sha256,generation,platform,digest)
);
