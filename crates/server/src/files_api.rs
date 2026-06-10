//! Local user files API (`/api/v0/files`).

use axum::body::Bytes;
use axum::extract::{Extension, Path as AxumPath, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use n3ur0n_storage::blobs;
use serde_json::json;

use crate::auth::{has_permission, AuthedUser};
use crate::auth::perm::{CAPS_BLOBS_READ, FILES_DELETE, FILES_READ};
use crate::http::AppState;

const CLIENT_ID_COOKIE: &str = "n3ur0n_client_id";

/// Local files API routes (mount under `/api/v0` with shared [`AppState`]).
pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/files", get(list_files).post(upload_file))
        .route("/files/*hash", get(get_file_meta).delete(delete_file))
        .route("/cap-jobs/blobs", get(list_cap_job_blobs))
}

async fn upload_file(
    State(state): State<AppState>,
    Extension(user): Extension<AuthedUser>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !has_permission(user.role, FILES_READ) {
        return (
            StatusCode::FORBIDDEN,
            axum::Json(json!({"error": "forbidden", "required_permission": FILES_READ})),
        )
            .into_response();
    }
    if body.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            axum::Json(json!({"error": "empty body"})),
        )
            .into_response();
    }
    let mime = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/octet-stream")
        .split(';')
        .next()
        .unwrap_or("application/octet-stream")
        .trim()
        .to_string();
    let client_id = client_id_from_cookie(&headers);
    match n3ur0n_node::blob_resolve::store_local_cache(
        &state.node,
        &body,
        &mime,
        Some(user.id),
        client_id.as_deref(),
    ) {
        Ok(blob_ref) => (
            StatusCode::CREATED,
            axum::Json(json!({
                "hash": blob_ref.hash,
                "size": blob_ref.size,
                "mime": blob_ref.mime,
            })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            axum::Json(json!({"error": e})),
        )
            .into_response(),
    }
}

fn client_id_from_cookie(headers: &axum::http::HeaderMap) -> Option<String> {
    headers
        .get(axum::http::header::COOKIE)
        .and_then(|h| h.to_str().ok())
        .and_then(|raw| {
            raw.split(';').find_map(|part| {
                let part = part.trim();
                part.strip_prefix(&format!("{CLIENT_ID_COOKIE}="))
                    .map(|v| v.to_string())
            })
        })
}

async fn list_files(
    State(state): State<AppState>,
    Extension(user): Extension<AuthedUser>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    if !has_permission(user.role, FILES_READ) {
        return (
            StatusCode::FORBIDDEN,
            axum::Json(json!({"error": "forbidden", "required_permission": FILES_READ})),
        )
            .into_response();
    }
    let client_id = client_id_from_cookie(&headers);
    match blobs::list_user_visible(state.node.db(), Some(user.id), client_id.as_deref(), 200) {
        Ok(rows) => {
            let body: Vec<_> = rows.iter().map(blobs::record_to_json).collect();
            axum::Json(json!({"files": body})).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            axum::Json(json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

async fn get_file_meta(
    State(state): State<AppState>,
    Extension(user): Extension<AuthedUser>,
    AxumPath(hash): AxumPath<String>,
    headers: axum::http::HeaderMap,
) -> Response {
    if !has_permission(user.role, FILES_READ) {
        return (
            StatusCode::FORBIDDEN,
            axum::Json(json!({"error": "forbidden", "required_permission": FILES_READ})),
        )
            .into_response();
    }
    let client_id = client_id_from_cookie(&headers);
    let record = match blobs::get(state.node.db(), &hash) {
        Ok(Some(r)) => r,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                axum::Json(json!({"error": "not found"})),
            )
                .into_response();
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                axum::Json(json!({"error": e.to_string()})),
            )
                .into_response();
        }
    };

    if !record.user_visible || !owns_record(&record, user.id, client_id.as_deref()) {
        return (
            StatusCode::NOT_FOUND,
            axum::Json(json!({"error": "not found"})),
        )
            .into_response();
    }

    // Stream bytes via local blob GET with forged ticket.
    let ticket = match crate::blobs::forge_local_get_ticket(
        state.node.keypair(),
        state.node.instance_id(),
        &hash,
        state.node.clock().as_ref(),
    ) {
        Ok(t) => t,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                axum::Json(json!({"error": e.to_string()})),
            )
                .into_response();
        }
    };
    let header_val = match n3ur0n_core::encode_ticket_wire(&ticket) {
        Ok(v) => v,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                axum::Json(json!({"error": e.to_string()})),
            )
                .into_response();
        }
    };

    let bytes = match std::fs::read(&record.storage_path) {
        Ok(b) => b,
        Err(_) => {
            return (
                StatusCode::NOT_FOUND,
                axum::Json(json!({"error": "file missing on disk"})),
            )
                .into_response();
        }
    };

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, record.mime.as_str())
        .header(header::CONTENT_LENGTH, record.size.to_string())
        .header(n3ur0n_core::BLOB_TICKET_HEADER, header_val)
        .body(axum::body::Body::from(bytes))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

async fn delete_file(
    State(state): State<AppState>,
    Extension(user): Extension<AuthedUser>,
    AxumPath(hash): AxumPath<String>,
    headers: axum::http::HeaderMap,
) -> Response {
    if !has_permission(user.role, FILES_DELETE) {
        return (
            StatusCode::FORBIDDEN,
            axum::Json(json!({"error": "forbidden", "required_permission": FILES_DELETE})),
        )
            .into_response();
    }
    let client_id = client_id_from_cookie(&headers);
    let record = match blobs::get(state.node.db(), &hash) {
        Ok(Some(r)) => r,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                axum::Json(json!({"error": "not found"})),
            )
                .into_response();
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                axum::Json(json!({"error": e.to_string()})),
            )
                .into_response();
        }
    };

    if !record.user_deletable || !owns_record(&record, user.id, client_id.as_deref()) {
        return (
            StatusCode::FORBIDDEN,
            axum::Json(json!({"error": "not deletable"})),
        )
            .into_response();
    }

    let _ = std::fs::remove_file(&record.storage_path);
    if let Err(e) = blobs::delete(state.node.db(), &hash) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            axum::Json(json!({"error": e.to_string()})),
        )
            .into_response();
    }
    StatusCode::NO_CONTENT.into_response()
}

async fn list_cap_job_blobs(
    State(state): State<AppState>,
    Extension(user): Extension<AuthedUser>,
) -> impl IntoResponse {
    if !has_permission(user.role, CAPS_BLOBS_READ) {
        return (
            StatusCode::FORBIDDEN,
            axum::Json(json!({"error": "forbidden", "required_permission": CAPS_BLOBS_READ})),
        )
            .into_response();
    }
    match blobs::list_cap_jobs(state.node.db(), 200) {
        Ok(rows) => {
            let body: Vec<_> = rows.iter().map(blobs::record_to_json).collect();
            axum::Json(json!({"blobs": body})).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            axum::Json(json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

fn owns_record(record: &blobs::BlobRecord, user_id: i64, client_id: Option<&str>) -> bool {
    if record.local_user_id == Some(user_id) {
        return true;
    }
    if let Some(cid) = client_id {
        if record.client_id.as_deref() == Some(cid) {
            return true;
        }
    }
    false
}
