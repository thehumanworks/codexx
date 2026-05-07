CREATE TABLE thread_queued_turns (
    queued_turn_id TEXT PRIMARY KEY NOT NULL,
    thread_id TEXT NOT NULL REFERENCES threads(id) ON DELETE CASCADE,
    turn_start_params_json TEXT NOT NULL,
    queue_order INTEGER NOT NULL,
    UNIQUE(thread_id, queue_order)
);
