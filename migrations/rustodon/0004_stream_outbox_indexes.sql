DROP INDEX rustodon.outbox_events_pending_idx;

CREATE INDEX outbox_events_pending_idx
    ON rustodon.outbox_events (id)
    WHERE dispatched_at IS NULL
      AND kind <> 'rustodon.mastodon.stream_event';

CREATE INDEX outbox_events_stream_id_idx
    ON rustodon.outbox_events (kind, id)
    WHERE kind = 'rustodon.mastodon.stream_event';

CREATE INDEX outbox_events_stream_created_at_idx
    ON rustodon.outbox_events (kind, created_at)
    WHERE kind = 'rustodon.mastodon.stream_event';
