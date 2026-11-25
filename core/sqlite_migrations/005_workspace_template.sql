CREATE TABLE workspace_template (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    description TEXT NOT NULL,
    github_repos TEXT NOT NULL CHECK (json_type(github_repos) = 'array')
);
INSERT INTO workspace_template VALUES (1, '', '[]');

-- Snapshot the default at creation so edits only affect future computers.
CREATE TABLE template_applications (
    sandbox_id TEXT PRIMARY KEY REFERENCES sandboxes(id) ON DELETE CASCADE,
    github_repos TEXT NOT NULL,
    claimed INTEGER NOT NULL DEFAULT 0
);
CREATE TRIGGER snapshot_workspace_template AFTER INSERT ON sandboxes
BEGIN
    INSERT INTO template_applications (sandbox_id, github_repos)
    SELECT NEW.id, github_repos FROM workspace_template WHERE json_array_length(github_repos) > 0;
END;
