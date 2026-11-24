CREATE TABLE provider_credentials (
    connection_id TEXT PRIMARY KEY REFERENCES provider_connections(id),
    provider TEXT NOT NULL,
    account TEXT NOT NULL,
    version TEXT NOT NULL,
    nonce BLOB NOT NULL CHECK (length(nonce) = 12),
    ciphertext BLOB NOT NULL
);
