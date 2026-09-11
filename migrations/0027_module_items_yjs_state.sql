-- Phase 31 (P31-005) — real-time collaborative editing. The server
-- only ever persists opaque Yjs CRDT bytes here (never projects them
-- back into ALM/content_blocks itself — no Rust equivalent of
-- y-prosemirror exists; the client does that projection via a
-- debounced PUT, see collab_hub.rs's own doc comment). Checkpointed on
-- an interval while >=1 collaborator is connected, plus once on the
-- last disconnect.
alter table "module_items" add column "yjs_state" bytea;
alter table "module_items" add column "yjs_state_updated_at" timestamptz;
