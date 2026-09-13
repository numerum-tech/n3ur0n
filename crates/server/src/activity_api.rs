//! Activity dashboard: what this node is doing and what it contributes.
//!
//! Not a log viewer. The question the view answers is "what is happening on
//! this instance right now", so the API serves *aggregates* — counts,
//! rankings, a pulse — and never individual rows. The journal behind it is
//! `audit_log`, written by `handler.rs` for every inbound exchange and by the
//! plan executor for every outbound call.
//!
//! Live updates are a periodic snapshot rather than a push from each write
//! site. A broadcast bus threaded from storage up through the node would be
//! more immediate and much more machinery; a small aggregate query on a local
//! SQLite every couple of seconds is what this actually needs, and it has the
//! property that a client which reconnects is instantly correct rather than
//! having to replay events it missed.

use std::collections::HashMap;
use std::convert::Infallible;
use std::time::Duration;

use axum::extract::{Query, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use tokio_stream::{Stream, StreamExt, wrappers::IntervalStream};
use n3ur0n_storage::audit::{self, Direction};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::http::AppState;

/// Windows the dashboard offers. Anything else is clamped into this range:
/// a window of a year would scan the whole table on every tick.
const MIN_WINDOW_SECS: i64 = 60;
const MAX_WINDOW_SECS: i64 = 7 * 24 * 3600;
const DEFAULT_WINDOW_SECS: i64 = 3600;

/// How many points the pulse carries, whatever the window. The chart has a
/// fixed width, so the bucket size follows the window instead.
const PULSE_BUCKETS: i64 = 30;

/// Cadence of the live stream.
const TICK: Duration = Duration::from_secs(2);

const TOP_N: i64 = 5;

#[derive(Debug, Deserialize)]
pub(crate) struct WindowQuery {
    #[serde(default)]
    window: Option<i64>,
}

impl WindowQuery {
    fn secs(&self) -> i64 {
        self.window
            .unwrap_or(DEFAULT_WINDOW_SECS)
            .clamp(MIN_WINDOW_SECS, MAX_WINDOW_SECS)
    }
}

#[derive(Debug, Serialize)]
struct Snapshot {
    now: i64,
    window_secs: i64,
    /// Calls this node answered — what it contributes to the network.
    served: audit::DirectionStats,
    /// Calls this node made — what it consumes from the network.
    called: audit::DirectionStats,
    /// Capabilities of this node the planner ran for its own user. Neither
    /// contribution nor consumption, and usually the bulk of the work.
    ran: audit::DirectionStats,
    /// Meta-verb exchanges answered: the discovery traffic. Counted apart
    /// from the capabilities so one bootstrap crawl does not outrank them.
    discovery: i64,
    /// Most solicited capabilities of this node, whoever asked.
    top_capabilities: Vec<Value>,
    /// Who calls us most.
    top_callers: Vec<Value>,
    /// Whom we call most.
    top_callees: Vec<Value>,
    /// Calls per bucket, oldest first, dense.
    pulse: Vec<audit::Bucket>,
}

fn snapshot(state: &AppState, window_secs: i64) -> Result<Snapshot, String> {
    let db = state.node.db();
    let now = state.node.clock().now().unix_timestamp();
    let since = now - window_secs;
    let aliases = alias_map(state);

    let label = |r: audit::Ranked| -> Value {
        json!({
            "key": r.key,
            "label": aliases.get(&r.key).cloned().unwrap_or_else(|| r.key.clone()),
            "calls": r.calls,
            "errors": r.errors,
        })
    };
    let plain = |r: audit::Ranked| -> Value {
        json!({"key": r.key, "label": r.key, "calls": r.calls, "errors": r.errors})
    };

    Ok(Snapshot {
        now,
        window_secs,
        served: audit::direction_stats(db, Direction::In, since).map_err(|e| e.to_string())?,
        called: audit::direction_stats(db, Direction::Out, since).map_err(|e| e.to_string())?,
        ran: audit::direction_stats(db, Direction::Local, since).map_err(|e| e.to_string())?,
        discovery: audit::meta_calls_since(db, since).map_err(|e| e.to_string())?,
        top_capabilities: audit::top_capabilities_solicited(db, since, TOP_N)
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(plain)
            .collect(),
        top_callers: audit::top_peers(db, Direction::In, since, TOP_N)
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(&label)
            .collect(),
        top_callees: audit::top_peers(db, Direction::Out, since, TOP_N)
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(&label)
            .collect(),
        pulse: audit::buckets(db, since, now + 1, (window_secs / PULSE_BUCKETS).max(1))
            .map_err(|e| e.to_string())?,
    })
}

/// Peer id → the readable handle the rest of the UI shows. A dashboard of
/// `n3:sfwxirorcnz3…` rows tells nobody anything.
fn alias_map(state: &AppState) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let self_id = state.node.instance_id().to_string();
    if let Some(alias) = state.node.alias() {
        out.insert(self_id.clone(), alias);
    } else {
        out.insert(self_id, "this instance".to_string());
    }
    if let Ok(peers) = n3ur0n_storage::peers::list(state.node.db(), 500) {
        for p in peers {
            if let Some(alias) = p.alias {
                out.insert(p.id, alias);
            }
        }
    }
    out
}

async fn activity_snapshot(
    State(state): State<AppState>,
    Query(q): Query<WindowQuery>,
) -> Response {
    match snapshot(&state, q.secs()) {
        Ok(s) => Json(s).into_response(),
        Err(e) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": "activity_unavailable", "message": e})),
        )
            .into_response(),
    }
}

async fn activity_stream(
    State(state): State<AppState>,
    Query(q): Query<WindowQuery>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let window = q.secs();
    // `tokio::time::interval` fires its first tick immediately, so a client is
    // never left looking at an empty dashboard for the length of one period.
    let stream = IntervalStream::new(tokio::time::interval(TICK)).map(move |_| {
        let payload = match snapshot(&state, window) {
            Ok(s) => serde_json::to_string(&s).unwrap_or_else(|_| "{}".into()),
            Err(e) => json!({"error": e}).to_string(),
        };
        Ok(Event::default().event("snapshot").data(payload))
    });
    Sse::new(stream).keep_alive(KeepAlive::default())
}

pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/activity", get(activity_snapshot))
        .route("/activity/stream", get(activity_stream))
}
