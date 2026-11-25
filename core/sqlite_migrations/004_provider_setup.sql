CREATE TABLE provider_setup (
    connection_id TEXT PRIMARY KEY REFERENCES provider_connections(id),
    configuration TEXT NOT NULL CHECK(json_valid(configuration))
);
