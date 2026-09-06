use std::collections::HashMap;
use std::sync::Mutex;

use tokio::sync::broadcast;
use uuid::Uuid;

// Port of canvas_ws.ts's broadcast mechanism. Bun's native
// `ws.subscribe(topic)`/`ws.publish(topic, data)` (1 topic per canvas
// session, topic = session id, publish excludes the sender) has no
// direct Axum equivalent — this is the explicit hub that replaces it:
// one `tokio::sync::broadcast` channel per session id, and every
// connection tags its own outgoing broadcasts with a unique
// `conn_id` so the send loop can skip re-delivering a message to its
// own sender (replicating Bun's publish-excludes-sender semantic,
// which `broadcast::Sender` does NOT do on its own — every subscriber,
// including the sender's own receiver, gets every message).
//
// Explicitly single-process, same as the Bun original: only reaches
// peers connected to THIS backend instance, no cross-instance fan-out.
// Channels are never pruned once created (a session realistically has
// at most 2 long-lived participants; matches the Bun original's own
// lack of any topic-cleanup logic — Bun's ws.publish/subscribe handles
// its own bookkeeping internally, so there was nothing to replicate).

#[derive(Debug, Clone)]
pub struct BroadcastEnvelope {
    pub origin_conn_id: Uuid,
    pub payload: String,
}

struct ConnInfo {
    session_id: Uuid,
    user_id: Uuid,
}

#[derive(Default)]
pub struct CanvasHub {
    channels: Mutex<HashMap<Uuid, broadcast::Sender<BroadcastEnvelope>>>,
    // Mirrors canvas_ws.ts's module-level `connections` Map (keyed by
    // ws.id there, by a generated conn_id here) — the side registry for
    // presence's "linear scan for a differing user_id already connected
    // to this session" check, since axum's WebSocket has no analogue to
    // Elysia's stable per-connection `ws.id` to key off of directly.
    connections: Mutex<HashMap<Uuid, ConnInfo>>,
}

impl CanvasHub {
    pub fn new() -> Self {
        Self::default()
    }

    // Registers a new connection and subscribes it to the session's
    // channel (creating one if this is the first connection for that
    // session). Returns (conn_id, peer_online, receiver) — peer_online
    // is computed BEFORE this connection is registered, matching the
    // Bun original's ordering (the scan happens before `connections.set`).
    pub fn connect(&self, session_id: Uuid, user_id: Uuid) -> (Uuid, bool, broadcast::Receiver<BroadcastEnvelope>) {
        let conn_id = Uuid::new_v4();

        let peer_online = {
            let connections = self.connections.lock().unwrap();
            connections.values().any(|c| c.session_id == session_id && c.user_id != user_id)
        };

        {
            let mut connections = self.connections.lock().unwrap();
            connections.insert(conn_id, ConnInfo { session_id, user_id });
        }

        let receiver = {
            let mut channels = self.channels.lock().unwrap();
            channels.entry(session_id).or_insert_with(|| broadcast::channel(256).0).subscribe()
        };

        (conn_id, peer_online, receiver)
    }

    // Removes the connection's registry entry. Returns (session_id,
    // user_id) if it was still registered (it always should be, unless
    // called twice) — the caller uses this to publish the
    // presence:offline message, same as the Bun original's close()
    // handler (which does NOT re-scan for other same-user connections
    // first — this is a faithfully-reproduced quirk: a 2nd tab for the
    // same user still triggers an offline announcement when the 1st
    // tab closes).
    pub fn disconnect(&self, conn_id: Uuid) -> Option<(Uuid, Uuid)> {
        let mut connections = self.connections.lock().unwrap();
        connections.remove(&conn_id).map(|c| (c.session_id, c.user_id))
    }

    pub fn publish(&self, session_id: Uuid, origin_conn_id: Uuid, payload: String) {
        let channels = self.channels.lock().unwrap();
        if let Some(sender) = channels.get(&session_id) {
            // No receivers (or all lagged out) is not an error — same as
            // Bun's ws.publish with 0 subscribers, a silent no-op.
            let _ = sender.send(BroadcastEnvelope { origin_conn_id, payload });
        }
    }
}
