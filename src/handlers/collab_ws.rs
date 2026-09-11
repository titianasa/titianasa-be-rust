use axum::{
    extract::{
        ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade},
        Path, Query, State,
    },
    http::{HeaderMap, HeaderValue},
    response::Response,
};
use futures_util::StreamExt;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use uuid::Uuid;
use yrs_axum::ws::{AxumSink, AxumStream};

use crate::services::module_item;
use crate::services::permissions::{Action, Resource};
use crate::state::AppState;

// Phase 31 (P31-005). Auth mirrors handlers/canvas_ws.rs exactly:
// browsers can't set custom headers on a WS handshake, so the JWT comes
// via a query param, reconstructed into a synthetic Authorization
// header so this reuses resolve_auth_context unchanged.

#[derive(Debug, serde::Deserialize)]
pub struct WsAuthQuery {
    pub token: Option<String>,
}

// GET /ws/module-items/{item_id}/collab?token=...
pub async fn ws_handler(ws: WebSocketUpgrade, Path(item_id): Path<Uuid>, Query(query): Query<WsAuthQuery>, State(state): State<Arc<AppState>>) -> Response {
    ws.on_upgrade(move |socket| handle_socket(socket, item_id, query.token, state))
}

async fn close_with(mut socket: WebSocket, code: u16, reason: &'static str) {
    let _ = socket.send(Message::Close(Some(CloseFrame { code, reason: reason.into() }))).await;
}

async fn handle_socket(socket: WebSocket, item_id: Uuid, token: Option<String>, state: Arc<AppState>) {
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

    // Phase 31 (P31-008) — an author-tier role always passes; otherwise
    // an explicit editor-level share grant on this item also passes
    // (an invited collaborator, not promoted to an author role org-wide).
    let is_author = crate::services::permissions::is_allowed(ctx.role.as_deref(), Resource::ModuleItem, Action::Create);
    let has_grant = !is_author
        && crate::services::resource_share::has_module_item_grant(&state.db, ctx.user_id, ctx.role.as_deref(), item_id, "editor")
            .await
            .unwrap_or(false);
    if !is_author && !has_grant {
        close_with(socket, 1008, "forbidden").await;
        return;
    }

    let status = match module_item::find_status(&state.db, item_id).await {
        Ok(Some(status)) => status,
        Ok(None) => {
            close_with(socket, 1008, "not_found").await;
            return;
        }
        Err(_) => {
            close_with(socket, 1011, "internal_error").await;
            return;
        }
    };
    // ADR-0008's "a published item can never be edited in place" rule,
    // extended to the collab socket — v1 is authoring-only, not
    // live-viewing of published content.
    if status == "published" || status == "archived" {
        close_with(socket, 1008, "item_not_editable").await;
        return;
    }

    let checkpoint_interval = Duration::from_secs(state.config.collab_checkpoint_interval_seconds as u64);
    let room = state.collab_hub.join(&state.db, item_id, checkpoint_interval).await;

    let (sink, stream) = socket.split();
    let sink = Arc::new(Mutex::new(AxumSink(sink)));
    let stream = AxumStream(stream);
    let subscription = room.broadcast_group.subscribe(sink, stream);
    let _ = subscription.completed().await;

    state.collab_hub.leave(&state.db, item_id).await;
}
