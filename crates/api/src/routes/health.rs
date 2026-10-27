use std::time::Duration;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use serde_json::{Value, json};

use crate::state::AppState;

/// Liveness: the process answers.
pub async fn live() -> Json<Value> {
    Json(json!({ "success": true, "data": { "status": "ok" } }))
}

/// Readiness: Postgres answers within two seconds. Nothing about the database is revealed.
pub async fn db(State(state): State<AppState>) -> (StatusCode, Json<Value>) {
    let ping = sqlx::query("SELECT 1").execute(&state.pool);
    match tokio::time::timeout(Duration::from_secs(2), ping).await {
        Ok(Ok(_)) => (
            StatusCode::OK,
            Json(json!({ "success": true, "data": { "status": "ok", "database": "up" } })),
        ),
        _ => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "success": false, "data": { "status": "error", "database": "down" } })),
        ),
    }
}
