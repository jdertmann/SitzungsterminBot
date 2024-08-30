CREATE TABLE courts (
    name TEXT NOT NULL PRIMARY KEY,
    full_name TEXT,
    last_update INTEGER NOT NULL -- unix timestamp
);

CREATE TABLE sessions (
    court TEXT NOT NULL,
    date TEXT NOT NULL, -- ISO8601 YYYY-MM-DD
    time TEXT NOT NULL,
    type TEXT NOT NULL,
    lawsuit TEXT NOT NULL,
    hall TEXT NOT NULL,
    reference TEXT NOT NULL,
    note TEXT NOT NULL
);

CREATE TABLE subscriptions (
    subscription_id INTEGER PRIMARY KEY NOT NULL,
    chat_id INTEGER NOT NULL,
    court TEXT NOT NULL,
    name TEXT NOT NULL,
    confirmation_sent INTEGER DEFAULT 0 NOT NULL,
    date_filter TEXT NOT NULL,
    reference_filter TEXT NOT NULL,
    UNIQUE (chat_id, name)
);
