CREATE TABLE books (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    url TEXT NOT NULL,
    description TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

CREATE TABLE versions (
    id INTEGER PRIMARY KEY,
    book_id INTEGER NOT NULL REFERENCES books (id),
    version TEXT NOT NULL,
    commit_sha TEXT NOT NULL,
    published_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    UNIQUE (book_id, version)
);

CREATE TABLE dependencies (
    version_id INTEGER NOT NULL REFERENCES versions (id),
    depends_on_book_id INTEGER NOT NULL REFERENCES books (id),
    PRIMARY KEY (version_id, depends_on_book_id)
);
