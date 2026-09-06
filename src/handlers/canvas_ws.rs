use axum::{
    extract::{
        ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade},
        Path, Query, State,
    },
    http::{HeaderMap, HeaderValue},
    response::Response,
};
use std::sync::Arc;
use uuid::Uuid;

use crate::services::canvas::{self, CanvasSession};
use crate::services::conversation::Conversation;
use crate::state::AppState;

// Port of canvas_ws.ts. Browsers can't set custom headers on a
// WebSocket handshake, so auth comes from a query param instead of the
// usual Authorization header — reconstructed into a synthetic HeaderMap
// so this reuses resolve_auth_context UNCHANGED rather than a parallel
// auth path, same trick the Bun original plays with a synthetic
// `Headers` object.
//
// document_updated/cursor_moved/selection_changed are never written as
// canvas_events rows — only comment_created/comment_resolved/
// mode_changed persist (enforced by the DB CHECK constraint too).

#[derive(Debug, serde::Deserialize)]
pub struct WsAuthQuery {
    pub token: Option<String>,
}

// GET /ws/canvas/{session_id}?token=...
pub async fn ws_handler(ws: WebSocketUpgrade, Path(session_id): Path<Uuid>, Query(query): Query<WsAuthQuery>, State(state): State<Arc<AppState>>) -> Response {
    ws.on_upgrade(move |socket| handle_socket(socket, session_id, query.token, state))
}

async fn close_with(mut socket: WebSocket, code: u16, reason: &'static str) {
    let _ = socket.send(Message::Close(Some(CloseFrame { code, reason: reason.into() }))).await;
}

async fn handle_socket(mut socket: WebSocket, session_id: Uuid, token: Option<String>, state: Arc<AppState>) {
    let Some(token) = token else {
        close_with(socket, 1008, "unauthorized").await;
        return;
    };

    let mut headers = HeaderMap::new();
    match HeaderValue::from_str(&format!("Bearer {token}")) {
        Ok(v) => {
            headers.insert("authorization", v);
        }
        Err(_) => {
            close_with(socket, 1008, "unauthorized").await;
            return;
        }
    }

    let ctx = match crate::middleware::auth::resolve_auth_context(&state, &headers).await {
        Ok(ctx) => ctx,
        Err(_) => {
            close_with(socket, 1008, "unauthorized").await;
            return;
        }
    };

    let authorized = match canvas::authorize_session(&state.db, &ctx, session_id).await {
        Ok(a) => a,
        Err(_) => {
            close_with(socket, 1008, "forbidden").await;
            return;
        }
    };
    let conversation = authorized.conversation;

    let (conn_id, peer_online, mut broadcast_rx) = state.canvas_hub.connect(session_id, ctx.user_id);

    let state_msg = serde_json::json!({
        "type": "state",
        "content": authorized.session.content,
        "version": authorized.session.version,
        "mode": authorized.session.mode,
        "status": authorized.session.status,
        "peer_online": peer_online,
    });
    if socket.send(Message::Text(state_msg.to_string().into())).await.is_err() {
        state.canvas_hub.disconnect(conn_id);
        return;
    }
    // Tell an already-connected peer this side just joined — publish
    // excludes the sender (via conn_id tagging), so this only ever
    // reaches the OTHER side.
    state.canvas_hub.publish(session_id, conn_id, serde_json::json!({"type": "presence", "actor_id": ctx.user_id, "online": true}).to_string());

    loop {
        tokio::select! {
            incoming = socket.recv() => {
                match incoming {
                    Some(Ok(Message::Text(text))) => {
                        if let Some(reply) = handle_incoming(&state, session_id, conn_id, ctx.user_id, &conversation, &text).await {
                            if socket.send(Message::Text(reply.into())).await.is_err() {
                                break;
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(_)) => {} // ping/pong/binary — ignored, matches parseMessage's silent-drop on anything not JSON-object-with-string-type
                    Some(Err(_)) => break,
                }
            }
            broadcast_msg = broadcast_rx.recv() => {
                match broadcast_msg {
                    Ok(envelope) if envelope.origin_conn_id != conn_id => {
                        if socket.send(Message::Text(envelope.payload.into())).await.is_err() {
                            break;
                        }
                    }
                    Ok(_) => {} // our own publish, skip (Bun's ws.publish excludes the sender)
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {} // slow consumer, skip missed messages and continue
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    }

    if let Some((session_id, user_id)) = state.canvas_hub.disconnect(conn_id) {
        state.canvas_hub.publish(session_id, conn_id, serde_json::json!({"type": "presence", "actor_id": user_id, "online": false}).to_string());
    }
}

// Returns a message to send directly back to THIS connection only (the
// "send to self AND publish" cases, and `error` responses) — broadcasts
// to the peer go straight through state.canvas_hub.publish from inside
// here since that's fire-and-forget and doesn't need the caller's
// socket handle.
async fn handle_incoming(state: &Arc<AppState>, session_id: Uuid, conn_id: Uuid, user_id: Uuid, conversation: &Conversation, raw: &str) -> Option<String> {
    let msg: serde_json::Value = serde_json::from_str(raw).ok()?;
    let msg_type = msg.get("type")?.as_str()?.to_string();

    let session: CanvasSession = canvas::find_session_by_id(&state.db, session_id).await.ok()??;
    if session.status != "active" {
        return None;
    }

    match msg_type.as_str() {
        "document_updated" => {
            let content = msg.get("content").and_then(|v| v.as_str()).unwrap_or("");
            match canvas::apply_document_update(&state.db, &session, conversation, user_id, content).await {
                Ok(updated) => {
                    let payload = serde_json::json!({"type": "document_updated", "content": updated.content, "version": updated.version, "actor_id": user_id}).to_string();
                    state.canvas_hub.publish(session_id, conn_id, payload);
                    None
                }
                Err(_) => Some(serde_json::json!({"type": "error", "detail": "document_update_rejected"}).to_string()),
            }
        }
        "cursor_moved" => {
            let position = msg.get("position").cloned().unwrap_or(serde_json::Value::Null);
            let payload = serde_json::json!({"type": "cursor_moved", "position": position, "actor_id": user_id}).to_string();
            state.canvas_hub.publish(session_id, conn_id, payload);
            None
        }
        "selection_changed" => {
            let start = msg.get("start").cloned().unwrap_or(serde_json::Value::Null);
            let end = msg.get("end").cloned().unwrap_or(serde_json::Value::Null);
            let payload = serde_json::json!({"type": "selection_changed", "start": start, "end": end, "actor_id": user_id}).to_string();
            state.canvas_hub.publish(session_id, conn_id, payload);
            None
        }
        "comment_created" => {
            let text = msg.get("text").and_then(|v| v.as_str()).unwrap_or("");
            let anchor_offset_value = msg.get("anchor_offset").cloned().unwrap_or(serde_json::Value::Null);
            let anchor_offset = anchor_offset_value.as_i64();
            match canvas::apply_comment(&state.db, &session, user_id, text, anchor_offset).await {
                Ok(event) => {
                    let payload = serde_json::json!({"type": "comment_created", "id": event.id, "actor_id": user_id, "text": text, "anchor_offset": anchor_offset_value, "created_at": event.created_at}).to_string();
                    state.canvas_hub.publish(session_id, conn_id, payload.clone());
                    Some(payload)
                }
                Err(_) => None,
            }
        }
        "comment_resolved" => {
            let comment_event_id = msg.get("comment_event_id").and_then(|v| v.as_str())?;
            match canvas::apply_comment_resolved(&state.db, &session, user_id, comment_event_id).await {
                Ok(event) => {
                    let payload = serde_json::json!({"type": "comment_resolved", "id": event.id, "comment_event_id": comment_event_id, "actor_id": user_id}).to_string();
                    state.canvas_hub.publish(session_id, conn_id, payload.clone());
                    Some(payload)
                }
                Err(_) => None,
            }
        }
        "mode_changed" => {
            let mode = msg.get("mode").and_then(|v| v.as_str()).unwrap_or("");
            match canvas::apply_mode_change(&state.db, &session, conversation, user_id, mode).await {
                Ok(updated) => {
                    let payload = serde_json::json!({"type": "mode_changed", "mode": updated.mode, "actor_id": user_id}).to_string();
                    state.canvas_hub.publish(session_id, conn_id, payload.clone());
                    Some(payload)
                }
                Err(_) => Some(serde_json::json!({"type": "error", "detail": "mode_change_rejected"}).to_string()),
            }
        }
        _ => None,
    }
}
