CREATE TABLE IF NOT EXISTS account (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    server TEXT,
    username TEXT
);
CREATE TABLE IF NOT EXISTS folder (
    id INTEGER PRIMARY KEY,
    account_id INTEGER NOT NULL REFERENCES account(id),
    path TEXT NOT NULL,
    uidvalidity INTEGER NOT NULL DEFAULT 0,
    last_uid INTEGER NOT NULL DEFAULT 0,
    UNIQUE(account_id, path)
);
CREATE TABLE IF NOT EXISTS message (
    id INTEGER PRIMARY KEY,
    sha256 TEXT NOT NULL UNIQUE,
    size INTEGER NOT NULL,
    date INTEGER,
    sender TEXT,
    recipients TEXT,
    subject TEXT,
    message_id TEXT,
    eml_path TEXT NOT NULL,
    archived_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_message_message_id ON message(message_id);
CREATE TABLE IF NOT EXISTS message_location (
    id INTEGER PRIMARY KEY,
    message_id INTEGER NOT NULL REFERENCES message(id),
    folder_id INTEGER NOT NULL REFERENCES folder(id),
    uid INTEGER,
    flags TEXT NOT NULL DEFAULT '',
    internal_date INTEGER
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_location_folder_uid
    ON message_location(folder_id, uid) WHERE uid IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_location_message ON message_location(message_id);
CREATE TABLE IF NOT EXISTS sync_run (
    id INTEGER PRIMARY KEY,
    profile TEXT,
    started_at INTEGER,
    finished_at INTEGER,
    added INTEGER,
    duplicates INTEGER,
    failed INTEGER,
    errors TEXT
);
CREATE TABLE IF NOT EXISTS export_state (
    target TEXT NOT NULL,
    sha256 TEXT NOT NULL,
    folder TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'done',
    PRIMARY KEY (target, sha256, folder)
);
