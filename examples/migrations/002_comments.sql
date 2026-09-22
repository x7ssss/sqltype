CREATE TABLE comments (
    id UUID PRIMARY KEY,
    post_id UUID NOT NULL,
    body TEXT NOT NULL
);
