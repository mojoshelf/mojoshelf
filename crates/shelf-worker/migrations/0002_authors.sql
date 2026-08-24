CREATE TABLE authors (
    id INTEGER PRIMARY KEY,
    github_id INTEGER NOT NULL UNIQUE,
    github_login TEXT NOT NULL,
    token_hash TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

ALTER TABLE books ADD COLUMN author_id INTEGER REFERENCES authors (id);
