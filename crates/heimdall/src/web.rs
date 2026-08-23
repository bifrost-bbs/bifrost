//! Web server router, REST API handlers, static file serving, and WebSocket endpoints.

use crate::app_mgr::AppManager;
use crate::auth::{AuthConfig, auth_middleware};
use crate::config_mgr::ConfigManager;
use crate::db_mgr::DatabaseManager;
use crate::logs::{LogBuffer, LogQuery};
use crate::stats::StatsManager;
use crate::supervisor::Supervisor;
use crate::web_client::handle_web_terminal_ws;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path as AxumPath, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router, middleware};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use std::path::PathBuf;
use std::sync::Arc;
use tower_http::cors::CorsLayer;

const EMBEDDED_INDEX_HTML: &str = include_str!("../web/index.html");
const EMBEDDED_STYLE_CSS: &str = include_str!("../web/style.css");
const EMBEDDED_APP_JS: &str = include_str!("../web/app.js");

#[derive(Clone)]
pub struct AppState {
    pub supervisor: Supervisor,
    pub config_mgr: Arc<ConfigManager>,
    pub app_mgr: Arc<AppManager>,
    pub stats_mgr: Arc<StatsManager>,
    pub db_mgr: Arc<DatabaseManager>,
    pub log_buffer: Arc<LogBuffer>,
    pub auth_config: AuthConfig,
    pub web_dir: Option<PathBuf>,
    pub radio_port: String,
}

#[derive(Debug, Deserialize)]
pub struct CrawlerQuery {
    pub steps: Option<usize>,
    pub delay: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct TuningQuery {
    pub command: String,
}

#[derive(Debug, Deserialize)]
pub struct TuningBody {
    pub args: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub struct PagingQuery {
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub struct AppFileQuery {
    pub path: String,
}

#[derive(Debug, Deserialize)]
pub struct SetKeyBody {
    pub value: String,
}

pub fn create_router(state: AppState) -> Router {
    let auth_conf = state.auth_config.clone();

    let api_routes = Router::new()
        // Supervisor
        .route("/api/supervisor/status", get(get_supervisor_status))
        .route("/api/supervisor/start_bbs", post(start_bbs_handler))
        .route("/api/supervisor/stop_bbs", post(stop_bbs_handler))
        .route("/api/supervisor/restart_bbs", post(restart_bbs_handler))
        .route("/api/supervisor/crawler", post(start_crawler_handler))
        .route("/api/supervisor/tuning", post(run_tuning_handler))
        // Logs
        .route("/api/logs", get(query_logs_handler))
        .route("/api/logs/clear", post(clear_logs_handler))
        // Config
        .route("/api/config", get(get_config_handler).post(save_config_handler))
        // Apps
        .route("/api/apps", get(list_apps_handler))
        .route("/api/apps/:app_id", get(get_app_detail_handler))
        .route("/api/apps/:app_id/files", get(list_app_files_handler))
        .route("/api/apps/:app_id/file_content", get(get_app_file_content_handler).post(save_app_file_content_handler))
        .route("/api/apps/:app_id/files/:filename", post(save_app_file_handler))
        // Database
        .route("/api/database/summary", get(get_database_summary_handler))
        .route("/api/database/tables", get(list_database_tables_handler))
        .route("/api/database/telemetry", get(get_database_telemetry_handler))
        .route("/api/database/backup", get(backup_database_handler))
        .route("/api/database/restore", post(restore_database_handler))
        .route("/api/database/reset", post(reset_database_handler))
        .route("/api/database/table/:namespace", get(get_database_table_handler).delete(clear_database_table_handler))
        .route("/api/database/table/:namespace/key/:key", get(get_database_key_handler).post(set_database_key_handler).delete(delete_database_key_handler))
        // Telemetry & Captures
        .route("/api/telemetry/summary", get(get_telemetry_summary_handler))
        .route("/api/telemetry/captures", get(get_captures_handler))
        .route("/api/telemetry/capture_summary", get(get_capture_summary_handler))
        // Apply Auth middleware to protected APIs if enabled
        .layer(middleware::from_fn_with_state(auth_conf.clone(), auth_middleware));

    let public_routes = Router::new()
        // Static assets
        .route("/", get(serve_index_html))
        .route("/index.html", get(serve_index_html))
        .route("/style.css", get(serve_style_css))
        .route("/app.js", get(serve_app_js))
        // WebSockets
        .route("/ws/logs", get(ws_logs_handler))
        .route("/ws/terminal", get(ws_terminal_handler));

    Router::new()
        .merge(public_routes)
        .merge(api_routes)
        .layer(CorsLayer::permissive())
        .with_state(state)
}

// --- STATIC FILE SERVING ---

async fn serve_index_html(State(state): State<AppState>) -> impl IntoResponse {
    if let Some(ref dir) = state.web_dir {
        let p = dir.join("index.html");
        if p.exists() {
            if let Ok(c) = std::fs::read_to_string(p) {
                return ([(header::CONTENT_TYPE, "text/html; charset=utf-8")], Html(c)).into_response();
            }
        }
    }
    ([(header::CONTENT_TYPE, "text/html; charset=utf-8")], Html(EMBEDDED_INDEX_HTML)).into_response()
}

async fn serve_style_css(State(state): State<AppState>) -> impl IntoResponse {
    if let Some(ref dir) = state.web_dir {
        let p = dir.join("style.css");
        if p.exists() {
            if let Ok(c) = std::fs::read_to_string(p) {
                return ([(header::CONTENT_TYPE, "text/css; charset=utf-8")], c).into_response();
            }
        }
    }
    ([(header::CONTENT_TYPE, "text/css; charset=utf-8")], EMBEDDED_STYLE_CSS).into_response()
}

async fn serve_app_js(State(state): State<AppState>) -> impl IntoResponse {
    if let Some(ref dir) = state.web_dir {
        let p = dir.join("app.js");
        if p.exists() {
            if let Ok(c) = std::fs::read_to_string(p) {
                return ([(header::CONTENT_TYPE, "application/javascript; charset=utf-8")], c).into_response();
            }
        }
    }
    ([(header::CONTENT_TYPE, "application/javascript; charset=utf-8")], EMBEDDED_APP_JS).into_response()
}

// --- REST HANDLERS ---

async fn get_supervisor_status(State(state): State<AppState>) -> impl IntoResponse {
    let list = state.supervisor.get_all_status().await;
    Json(list)
}

async fn start_bbs_handler(State(state): State<AppState>) -> Result<impl IntoResponse, (StatusCode, String)> {
    let cfg_path = state.config_mgr.get_config_path().to_string_lossy().to_string();
    state
        .supervisor
        .start_bbs(Some(&cfg_path), Some("captured_packets"))
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(serde_json::json!({ "status": "started" })))
}

async fn stop_bbs_handler(State(state): State<AppState>) -> Result<impl IntoResponse, (StatusCode, String)> {
    state
        .supervisor
        .stop_bbs()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(serde_json::json!({ "status": "stopped" })))
}

async fn restart_bbs_handler(State(state): State<AppState>) -> Result<impl IntoResponse, (StatusCode, String)> {
    let cfg_path = state.config_mgr.get_config_path().to_string_lossy().to_string();
    state
        .supervisor
        .restart_bbs(Some(&cfg_path), Some("captured_packets"))
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(serde_json::json!({ "status": "restarted" })))
}

async fn start_crawler_handler(
    State(state): State<AppState>,
    Query(q): Query<CrawlerQuery>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let steps = q.steps.unwrap_or(100);
    let delay = q.delay.unwrap_or(50);
    state
        .supervisor
        .start_crawler(steps, delay)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(serde_json::json!({ "status": "crawler_started", "steps": steps })))
}

async fn run_tuning_handler(
    State(state): State<AppState>,
    Query(q): Query<TuningQuery>,
    body: Option<Json<TuningBody>>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let extra = body.and_then(|b| b.args.clone()).unwrap_or_default();
    let str_args: Vec<&str> = extra.iter().map(|s| s.as_str()).collect();

    state
        .supervisor
        .run_tuning(&q.command, &str_args)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(serde_json::json!({ "status": "tuning_started", "command": q.command })))
}

async fn query_logs_handler(
    State(state): State<AppState>,
    Query(q): Query<LogQuery>,
) -> impl IntoResponse {
    let entries = state.log_buffer.query(&q);
    Json(entries)
}

async fn clear_logs_handler(State(state): State<AppState>) -> impl IntoResponse {
    state.log_buffer.clear();
    Json(serde_json::json!({ "status": "cleared" }))
}

async fn get_config_handler(State(state): State<AppState>) -> Result<impl IntoResponse, (StatusCode, String)> {
    let resp = state
        .config_mgr
        .get_response()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(resp))
}

async fn save_config_handler(
    State(state): State<AppState>,
    body: String,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let parsed = state
        .config_mgr
        .save_raw_toml(&body)
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    Ok(Json(parsed))
}

async fn list_apps_handler(State(state): State<AppState>) -> impl IntoResponse {
    let cfg = state.config_mgr.get_config();
    let apps = state.app_mgr.list_apps(&cfg.apps.enabled, &cfg.apps.main_app);
    Json(apps)
}

async fn get_app_detail_handler(
    State(state): State<AppState>,
    AxumPath(app_id): AxumPath<String>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let cfg = state.config_mgr.get_config();
    let detail = state
        .app_mgr
        .get_app_detail(&app_id, &cfg.apps.enabled, &cfg.apps.main_app)
        .map_err(|e| (StatusCode::NOT_FOUND, e.to_string()))?;
    Ok(Json(detail))
}

async fn list_app_files_handler(
    State(state): State<AppState>,
    AxumPath(app_id): AxumPath<String>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let files = state
        .app_mgr
        .list_app_files(&app_id)
        .map_err(|e| (StatusCode::NOT_FOUND, e.to_string()))?;
    Ok(Json(files))
}

async fn get_app_file_content_handler(
    State(state): State<AppState>,
    AxumPath(app_id): AxumPath<String>,
    Query(q): Query<AppFileQuery>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let content = state
        .app_mgr
        .read_app_file(&app_id, &q.path)
        .map_err(|e| (StatusCode::NOT_FOUND, e.to_string()))?;
    Ok(content)
}

async fn save_app_file_content_handler(
    State(state): State<AppState>,
    AxumPath(app_id): AxumPath<String>,
    Query(q): Query<AppFileQuery>,
    body: String,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    state
        .app_mgr
        .save_app_file(&app_id, &q.path, &body)
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    Ok(Json(serde_json::json!({ "status": "saved", "path": q.path })))
}

async fn save_app_file_handler(
    State(state): State<AppState>,
    AxumPath((app_id, filename)): AxumPath<(String, String)>,
    body: String,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    state
        .app_mgr
        .save_app_file(&app_id, &filename, &body)
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    Ok(Json(serde_json::json!({ "status": "saved" })))
}

async fn get_telemetry_summary_handler(State(state): State<AppState>) -> impl IntoResponse {
    let cap_summary = state.stats_mgr.get_capture_summary();
    let cfg = state.config_mgr.get_config();
    let db_tel = state.db_mgr.telemetry().ok();
    let db_summary = state.db_mgr.summary().ok();

    let snapshot = serde_json::json!({
        "active_sessions": 0,
        "unique_users_24h": cap_summary.unique_users_count,
        "duty_cycle_percent": 0.0,
        "total_packets_sent": cap_summary.tx_count,
        "total_packets_received": cap_summary.rx_count,
        "total_raw_bytes_sent": cap_summary.total_raw_bytes,
        "total_compressed_bytes_sent": cap_summary.total_comp_bytes,
        "avg_raw_bytes_per_packet": cap_summary.avg_raw_bytes,
        "avg_comp_bytes_per_packet": cap_summary.avg_comp_bytes,
        "avg_bytes_per_packet": cap_summary.avg_bytes_per_packet,
        "avg_bytes_per_packet_per_user": cap_summary.avg_bytes_per_packet_per_user,
        "compression_savings_percent": cap_summary.net_savings_percent,
        "max_duty_cycle_limit": cfg.rate_limiter.max_duty_cycle_percent,
        "database": db_tel,
        "database_summary": db_summary,
    });

    Json(snapshot)
}

async fn get_captures_handler(
    State(state): State<AppState>,
    Query(q): Query<PagingQuery>,
) -> impl IntoResponse {
    let (rows, total) = state.stats_mgr.get_captured_packets(q.limit, q.offset);
    Json(serde_json::json!({ "rows": rows, "total": total }))
}

async fn get_capture_summary_handler(State(state): State<AppState>) -> impl IntoResponse {
    let summary = state.stats_mgr.get_capture_summary();
    Json(summary)
}

async fn get_database_summary_handler(
    State(state): State<AppState>,
) -> Result<Json<crate::db_mgr::DatabaseSummary>, (StatusCode, String)> {
    state
        .db_mgr
        .summary()
        .map(Json)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

async fn list_database_tables_handler(
    State(state): State<AppState>,
) -> Result<Json<Vec<crate::db_mgr::TableSummary>>, (StatusCode, String)> {
    state
        .db_mgr
        .list_tables()
        .map(Json)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

async fn get_database_telemetry_handler(
    State(state): State<AppState>,
) -> Result<Json<bifrost_bbs::DbTelemetryStats>, (StatusCode, String)> {
    state
        .db_mgr
        .telemetry()
        .map(Json)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

async fn backup_database_handler(
    State(state): State<AppState>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let bytes = state
        .db_mgr
        .backup_db()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let filename = format!(
        "database_backup_{}.db",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    );

    let mut headers = axum::http::HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, "application/octet-stream".parse().unwrap());
    headers.insert(
        header::CONTENT_DISPOSITION,
        format!("attachment; filename=\"{}\"", filename).parse().unwrap(),
    );

    Ok((headers, bytes))
}

async fn restore_database_handler(
    State(state): State<AppState>,
    body: axum::body::Bytes,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    if body.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "Empty backup data".to_string()));
    }
    state
        .db_mgr
        .restore_db(&body)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(serde_json::json!({ "status": "ok", "restored": true, "bytes": body.len() })))
}

async fn reset_database_handler(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    state
        .db_mgr
        .reset_db()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(serde_json::json!({ "status": "ok", "reset": true })))
}

async fn get_database_table_handler(
    State(state): State<AppState>,
    AxumPath(namespace): AxumPath<String>,
) -> Result<Json<Vec<crate::db_mgr::KeyValueEntry>>, (StatusCode, String)> {
    state
        .db_mgr
        .get_table_entries(&namespace)
        .map(Json)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

async fn clear_database_table_handler(
    State(state): State<AppState>,
    AxumPath(namespace): AxumPath<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let count = state
        .db_mgr
        .clear_table(&namespace)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(serde_json::json!({ "status": "ok", "cleared_records": count })))
}

async fn get_database_key_handler(
    State(state): State<AppState>,
    AxumPath((namespace, key)): AxumPath<(String, String)>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let val = state
        .db_mgr
        .get_key(&namespace, &key)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(serde_json::json!({ "namespace": namespace, "key": key, "value": val })))
}

async fn set_database_key_handler(
    State(state): State<AppState>,
    AxumPath((namespace, key)): AxumPath<(String, String)>,
    Json(body): Json<SetKeyBody>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    state
        .db_mgr
        .set_key(&namespace, &key, &body.value)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(serde_json::json!({ "status": "ok", "namespace": namespace, "key": key })))
}

async fn delete_database_key_handler(
    State(state): State<AppState>,
    AxumPath((namespace, key)): AxumPath<(String, String)>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    state
        .db_mgr
        .delete_key(&namespace, &key)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(serde_json::json!({ "status": "ok", "deleted": true })))
}

// --- WEBSOCKET HANDLERS ---

async fn ws_logs_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> Response {
    ws.on_upgrade(move |socket| handle_logs_ws(socket, state.log_buffer))
}

async fn handle_logs_ws(socket: WebSocket, log_buffer: Arc<LogBuffer>) {
    let (mut sender, _) = socket.split();
    let mut rx = log_buffer.subscribe();

    while let Ok(entry) = rx.recv().await {
        if let Ok(json_str) = serde_json::to_string(&entry) {
            if sender.send(Message::Text(json_str)).await.is_err() {
                break;
            }
        }
    }
}

async fn ws_terminal_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> Response {
    let port = state.radio_port.clone();
    let log_buf = state.log_buffer.clone();
    ws.on_upgrade(move |socket| handle_web_terminal_ws(socket, port, log_buf))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    #[tokio::test]
    async fn test_web_routes_and_static_fallback() {
        let temp_dir = std::env::temp_dir().join(format!("heimdall_web_test_{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&temp_dir).unwrap();

        let log_buf = Arc::new(LogBuffer::default());
        let supervisor = Supervisor::new(&temp_dir, log_buf.clone());
        let config_mgr = Arc::new(ConfigManager::new(temp_dir.join("config.toml")));
        let app_mgr = Arc::new(AppManager::new(temp_dir.join("apps")));
        let stats_mgr = Arc::new(StatsManager::new(temp_dir.join("captures")));
        let db_mgr = Arc::new(DatabaseManager::new(temp_dir.join("database.db")));

        let state = AppState {
            supervisor,
            config_mgr,
            app_mgr,
            stats_mgr,
            db_mgr,
            log_buffer: log_buf,
            auth_config: AuthConfig::default(),
            web_dir: None,
            radio_port: "127.0.0.1:8088".to_string(),
        };

        let app = create_router(state);

        // Test GET /
        let req = Request::builder().uri("/").body(Body::empty()).unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // Test GET /style.css
        let req_css = Request::builder().uri("/style.css").body(Body::empty()).unwrap();
        let resp_css = app.clone().oneshot(req_css).await.unwrap();
        assert_eq!(resp_css.status(), StatusCode::OK);

        // Test GET /api/supervisor/status
        let req_status = Request::builder().uri("/api/supervisor/status").body(Body::empty()).unwrap();
        let resp_status = app.clone().oneshot(req_status).await.unwrap();
        assert_eq!(resp_status.status(), StatusCode::OK);

        // Test GET /api/config
        let req_cfg = Request::builder().uri("/api/config").body(Body::empty()).unwrap();
        let resp_cfg = app.clone().oneshot(req_cfg).await.unwrap();
        assert_eq!(resp_cfg.status(), StatusCode::OK);

        // Test POST /api/config
        let valid_toml = r#"
log_level = "debug"

[rate_limiter]
max_packets_per_minute = 45
max_burst_packets = 4
inter_packet_guard_ms = 350
max_duty_cycle_percent = 1.0
duty_cycle_window_secs = 3600

[asset_broadcaster]
enable_on_demand_broadcast = true
max_asset_broadcast_duty_cycle = 0.15

[form_colors]
submit_fg = 14
submit_bg = 4
field_fg = 0
field_bg = 14

admin_nodes = []

[apps]
main_app = "main_menu"
enabled = ["main_menu"]

[packet_capture]
enabled = false
directory = "captured_packets"
"#;
        let req_save_cfg = Request::builder().method("POST").uri("/api/config").body(Body::from(valid_toml)).unwrap();
        let resp_save_cfg = app.clone().oneshot(req_save_cfg).await.unwrap();
        assert_eq!(resp_save_cfg.status(), StatusCode::OK);

        // Test GET /api/apps
        let req_apps = Request::builder().uri("/api/apps").body(Body::empty()).unwrap();
        let resp_apps = app.clone().oneshot(req_apps).await.unwrap();
        assert_eq!(resp_apps.status(), StatusCode::OK);

        // Test GET /api/telemetry/summary
        let req_tel = Request::builder().uri("/api/telemetry/summary").body(Body::empty()).unwrap();
        let resp_tel = app.clone().oneshot(req_tel).await.unwrap();
        assert_eq!(resp_tel.status(), StatusCode::OK);

        // Test GET /api/telemetry/captures
        let req_cap = Request::builder().uri("/api/telemetry/captures").body(Body::empty()).unwrap();
        let resp_cap = app.clone().oneshot(req_cap).await.unwrap();
        assert_eq!(resp_cap.status(), StatusCode::OK);

        // Test GET /api/telemetry/capture_summary
        let req_cap_sum = Request::builder().uri("/api/telemetry/capture_summary").body(Body::empty()).unwrap();
        let resp_cap_sum = app.clone().oneshot(req_cap_sum).await.unwrap();
        assert_eq!(resp_cap_sum.status(), StatusCode::OK);

        // Test GET /api/logs
        let req_logs = Request::builder().uri("/api/logs").body(Body::empty()).unwrap();
        let resp_logs = app.clone().oneshot(req_logs).await.unwrap();
        assert_eq!(resp_logs.status(), StatusCode::OK);

        // Test GET /api/database/summary
        let req_db_sum = Request::builder().uri("/api/database/summary").body(Body::empty()).unwrap();
        let resp_db_sum = app.clone().oneshot(req_db_sum).await.unwrap();
        assert_eq!(resp_db_sum.status(), StatusCode::OK);

        // Test POST /api/database/table/:namespace/key/:key
        let set_body = serde_json::json!({ "value": "{\"test\": 123}" }).to_string();
        let req_set_db = Request::builder()
            .method("POST")
            .uri("/api/database/table/unit_test_ns/key/key1")
            .header("Content-Type", "application/json")
            .body(Body::from(set_body))
            .unwrap();
        let resp_set_db = app.clone().oneshot(req_set_db).await.unwrap();
        assert_eq!(resp_set_db.status(), StatusCode::OK);

        // Test GET /api/database/tables
        let req_db_tbls = Request::builder().uri("/api/database/tables").body(Body::empty()).unwrap();
        let resp_db_tbls = app.clone().oneshot(req_db_tbls).await.unwrap();
        assert_eq!(resp_db_tbls.status(), StatusCode::OK);

        // Test GET /api/database/table/:namespace
        let req_db_rows = Request::builder().uri("/api/database/table/unit_test_ns").body(Body::empty()).unwrap();
        let resp_db_rows = app.clone().oneshot(req_db_rows).await.unwrap();
        assert_eq!(resp_db_rows.status(), StatusCode::OK);

        // Test GET /api/database/telemetry
        let req_db_tel = Request::builder().uri("/api/database/telemetry").body(Body::empty()).unwrap();
        let resp_db_tel = app.clone().oneshot(req_db_tel).await.unwrap();
        assert_eq!(resp_db_tel.status(), StatusCode::OK);

        // Test GET /api/database/backup
        let req_db_bak = Request::builder().uri("/api/database/backup").body(Body::empty()).unwrap();
        let resp_db_bak = app.clone().oneshot(req_db_bak).await.unwrap();
        assert_eq!(resp_db_bak.status(), StatusCode::OK);

        // Test POST /api/database/reset
        let req_db_reset = Request::builder().method("POST").uri("/api/database/reset").body(Body::empty()).unwrap();
        let resp_db_reset = app.clone().oneshot(req_db_reset).await.unwrap();
        assert_eq!(resp_db_reset.status(), StatusCode::OK);

        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
