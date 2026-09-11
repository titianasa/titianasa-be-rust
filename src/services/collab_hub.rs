use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use sqlx::PgPool;
use tokio::sync::{Mutex, RwLock};
use tokio::task::JoinHandle;
use uuid::Uuid;
use yrs::sync::Awareness;
use yrs::updates::decoder::Decode;
use yrs::{Doc, ReadTxn, StateVector, Transact, Update};
use yrs_axum::broadcast::BroadcastGroup;
use yrs_axum::AwarenessRef;

// Phase 31 (P31-005) — real-time collaborative editing of module_items.
// `yrs-axum`'s `BroadcastGroup` already implements the full y-sync wire
// protocol fan-out (doc updates AND awareness/cursor state) — no
// hand-rolled message routing needed, unlike CanvasHub (P26-001, a much
// simpler last-write-wins text field with its own bespoke JSON
// protocol). One room per module_item id.
//
// Deliberate divergence from CanvasHub's "never pruned" pattern: that's
// fine there (a canvas session realistically has at most 2 long-lived
// participants), but doesn't hold here — a room must be evicted (after
// a final checkpoint flush) once its last subscriber leaves, or memory
// grows unboundedly across however many module_items get opened over
// the process's lifetime.

pub struct Room {
    pub awareness: AwarenessRef,
    pub broadcast_group: Arc<BroadcastGroup>,
    subscribers: AtomicUsize,
    checkpoint_task: JoinHandle<()>,
}

#[derive(Default)]
pub struct CollabHub {
    rooms: Mutex<HashMap<Uuid, Arc<Room>>>,
}

async fn checkpoint(pool: &PgPool, item_id: Uuid, awareness: &AwarenessRef) {
    let update = {
        let awareness = awareness.read().await;
        let txn = awareness.doc().transact();
        txn.encode_state_as_update_v1(&StateVector::default())
    };
    if let Err(e) = sqlx::query!(r#"update module_items set yjs_state = $2, yjs_state_updated_at = now() where id = $1"#, item_id, update).execute(pool).await {
        tracing::error!(error = ?e, %item_id, "failed to checkpoint yjs_state");
    }
}

impl CollabHub {
    pub fn new() -> Self {
        Self::default()
    }

    // Joins (creating on first connect) the room for `item_id`,
    // incrementing its subscriber count. The first connection loads any
    // existing `yjs_state` from Postgres into a fresh yrs::Doc before
    // constructing the BroadcastGroup, so a reconnect after every
    // client disconnected picks up where the last checkpoint left off
    // rather than starting from an empty document.
    pub async fn join(&self, pool: &PgPool, item_id: Uuid, checkpoint_interval: Duration) -> Arc<Room> {
        let mut rooms = self.rooms.lock().await;
        if let Some(room) = rooms.get(&item_id) {
            room.subscribers.fetch_add(1, Ordering::SeqCst);
            return room.clone();
        }

        let existing_state: Option<Vec<u8>> = sqlx::query_scalar!(r#"select yjs_state from module_items where id = $1"#, item_id)
            .fetch_optional(pool)
            .await
            .ok()
            .flatten()
            .flatten();

        let doc = Doc::new();
        if let Some(bytes) = existing_state {
            match Update::decode_v1(&bytes) {
                Ok(update) => {
                    let mut txn = doc.transact_mut();
                    txn.apply_update(update);
                }
                Err(e) => tracing::error!(error = ?e, %item_id, "failed to decode persisted yjs_state"),
            }
        }

        let awareness: AwarenessRef = Arc::new(RwLock::new(Awareness::new(doc)));
        let broadcast_group = Arc::new(BroadcastGroup::new(awareness.clone(), 32).await);

        let checkpoint_task = {
            let pool = pool.clone();
            let awareness = awareness.clone();
            tokio::spawn(async move {
                let mut interval = tokio::time::interval(checkpoint_interval);
                interval.tick().await; // first tick fires immediately — skip it, nothing to save yet
                loop {
                    interval.tick().await;
                    checkpoint(&pool, item_id, &awareness).await;
                }
            })
        };

        let room = Arc::new(Room { awareness, broadcast_group, subscribers: AtomicUsize::new(1), checkpoint_task });
        rooms.insert(item_id, room.clone());
        room
    }

    // Called on disconnect. When the last subscriber leaves: one final
    // checkpoint flush (so a crashed/killed interval task never loses
    // the tail of a session), stop the interval task, evict the room.
    pub async fn leave(&self, pool: &PgPool, item_id: Uuid) {
        let mut rooms = self.rooms.lock().await;
        let Some(room) = rooms.get(&item_id) else { return };
        let remaining = room.subscribers.fetch_sub(1, Ordering::SeqCst) - 1;
        if remaining == 0 {
            checkpoint(pool, item_id, &room.awareness).await;
            room.checkpoint_task.abort();
            rooms.remove(&item_id);
        }
    }
}
