//! Web server router, REST API handlers, static file serving, user management, and WebSocket endpoints.

use crate::app_mgr::AppManager;
use crate::auth::{AuthConfig, Session, SessionManager, TokenQuery};
use crate::config_mgr::ConfigManager;
use crate::db_mgr::DatabaseManager;
use crate::logs::{LogBuffer, LogQuery};
use crate::stats::StatsManager;
use crate::supervisor::Supervisor;
use crate::user_mgr::{
    UserInfo, UserManager, ALL_HEIMDALL_PERMISSIONS, PERM_ADMIN, PERM_HEIMDALL_LOGIN,
    PERM_HEIMDALL_USERS,
};
use crate::web_client::handle_web_terminal_ws;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path as AxumPath, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use bifrost_bbs::BbsNetworkRegistryManager;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
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
    pub user_mgr: Arc<UserManager>,
    pub session_mgr: Arc<SessionManager>,
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

#[derive(Debug, Deserialize)]
pub struct SetupBody {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Deserialize)]
pub struct LoginBody {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Deserialize)]
pub struct ChangePasswordBody {
    pub old_password: String,
    pub new_password: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateUserBody {
    pub username: String,
    pub password: String,
    pub permissions: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub struct UpdatePermissionsBody {
    pub permissions: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct ResetPasswordBody {
    pub new_password: String,
}

#[derive(Debug, Deserialize)]
pub struct CatalogQuery {
    pub refresh: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct InstallCatalogAppBody {
    pub app_id: String,
    pub tag: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UninstallCatalogAppBody {
    pub app_id: String,
}

#[derive(Debug, Deserialize)]
pub struct ToggleAppBody {
    pub enabled: bool,
}

#[derive(Debug, Deserialize)]
pub struct NetworkQuery {
    pub refresh: Option<bool>,
    pub q: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct TestPingBody {
    pub host: String,
    pub port: u16,
}

#[derive(Debug, Serialize)]
pub struct AuthStatusResponse {
    pub setup_required: bool,
    pub authenticated: bool,
    pub user: Option<UserInfo>,
    pub impersonating: Option<crate::auth::ImpersonationInfo>,
    pub all_permissions: Vec<PermissionMeta>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PermissionMeta {
    pub id: String,
    pub name: String,
    pub description: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AuthResponse {
    pub token: String,
    pub user: UserInfo,
}

pub fn create_router(state: AppState) -> Router {
    let api_routes = Router::new()
        // Auth & Identity
        .route("/api/auth/status", get(get_auth_status_handler))
        .route("/api/auth/setup", post(setup_admin_handler))
        .route("/api/auth/login", post(login_handler))
        .route("/api/auth/logout", post(logout_handler))
        .route("/api/auth/me", get(get_me_handler))
        .route(
            "/api/auth/change_password",
            post(change_my_password_handler),
        )
        .route(
            "/api/auth/stop_impersonating",
            post(stop_impersonating_handler),
        )
        // User Management
        .route(
            "/api/users",
            get(list_users_handler).post(create_user_handler),
        )
        .route(
            "/api/users/:node_id",
            get(get_user_handler).delete(delete_user_handler),
        )
        .route(
            "/api/users/:node_id/permissions",
            put(update_user_permissions_handler),
        )
        .route(
            "/api/users/:node_id/reset_password",
            post(reset_user_password_handler),
        )
        .route(
            "/api/users/:node_id/impersonate",
            post(impersonate_user_handler),
        )
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
        .route(
            "/api/config",
            get(get_config_handler).post(save_config_handler),
        )
        // Apps & Catalog
        .route("/api/apps", get(list_apps_handler))
        .route("/api/apps/:app_id", get(get_app_detail_handler))
        .route("/api/apps/:app_id/toggle", post(toggle_app_handler))
        .route("/api/apps/:app_id/files", get(list_app_files_handler))
        .route(
            "/api/apps/:app_id/file_content",
            get(get_app_file_content_handler).post(save_app_file_content_handler),
        )
        .route(
            "/api/apps/:app_id/files/:filename",
            post(save_app_file_handler),
        )
        .route("/api/catalog", get(get_catalog_handler))
        .route("/api/catalog/install", post(install_catalog_app_handler))
        .route(
            "/api/catalog/uninstall",
            post(uninstall_catalog_app_handler),
        )
        // Database
        .route("/api/database/summary", get(get_database_summary_handler))
        .route("/api/database/tables", get(list_database_tables_handler))
        .route(
            "/api/database/telemetry",
            get(get_database_telemetry_handler),
        )
        .route("/api/database/backup", get(backup_database_handler))
        .route("/api/database/restore", post(restore_database_handler))
        .route("/api/database/reset", post(reset_database_handler))
        .route(
            "/api/database/table/:namespace",
            get(get_database_table_handler).delete(clear_database_table_handler),
        )
        .route(
            "/api/database/table/:namespace/key/:key",
            get(get_database_key_handler)
                .post(set_database_key_handler)
                .delete(delete_database_key_handler),
        )
        // Telemetry
        .route("/api/telemetry/summary", get(get_telemetry_summary_handler))
        .route("/api/telemetry/captures", get(get_captures_handler))
        .route(
            "/api/telemetry/capture_summary",
            get(get_capture_summary_handler),
        )
        // Multi-BBS Network Registry
        .route("/api/network", get(get_network_registry_handler))
        .route("/api/network/sync", post(sync_network_registry_handler))
        .route("/api/network/test", post(test_network_ping_handler))
        // WebSockets
        .route("/ws/logs", get(ws_logs_handler))
        .route("/ws/terminal", get(ws_terminal_handler));

    // Static asset handlers
    Router::new()
        .merge(api_routes)
        .route("/", get(serve_index_html))
        .route("/index.html", get(serve_index_html))
        .route("/style.css", get(serve_style_css))
        .route("/app.js", get(serve_app_js))
        .layer(CorsLayer::permissive())
        .with_state(state)
}

// --- AUTH HELPERS ---

fn extract_session(
    state: &AppState,
    headers: &HeaderMap,
    query_tok: Option<&str>,
) -> Option<Session> {
    let tok = state.session_mgr.extract_token(headers, query_tok)?;
    state.session_mgr.get_session(&tok)
}

fn require_auth_session(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<Session, (StatusCode, String)> {
    if let Some(session) = extract_session(state, headers, None) {
        return Ok(session);
    }
    // If auth is completely disabled in config AND no users exist, allow fallback
    if !state.auth_config.enabled && state.user_mgr.is_setup_required().unwrap_or(false) {
        return Ok(Session {
            token: "anon".to_string(),
            user_id: [0u8; 32],
            user_id_hex: "0000000000000000000000000000000000000000000000000000000000000000"
                .to_string(),
            username: "Admin (Open Mode)".to_string(),
            permissions: vec![PERM_ADMIN.to_string()],
            is_admin: true,
            impersonating: None,
            created_instant: None,
            created_at: 0,
        });
    }
    Err((
        StatusCode::UNAUTHORIZED,
        "Authentication required".to_string(),
    ))
}

fn require_perm(
    state: &AppState,
    headers: &HeaderMap,
    perm: &str,
) -> Result<Session, (StatusCode, String)> {
    let session = require_auth_session(state, headers)?;
    if !session.has_permission(perm) {
        return Err((
            StatusCode::FORBIDDEN,
            format!("Access Denied: Missing permission '{}'", perm),
        ));
    }
    Ok(session)
}

// --- AUTH & USER HANDLERS ---

async fn get_auth_status_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<AuthStatusResponse>, (StatusCode, String)> {
    let setup_required = state.user_mgr.is_setup_required().unwrap_or(false);
    let session = extract_session(&state, &headers, None);

    let all_perms = ALL_HEIMDALL_PERMISSIONS
        .iter()
        .map(|(id, name, desc)| PermissionMeta {
            id: id.to_string(),
            name: name.to_string(),
            description: desc.to_string(),
        })
        .collect();

    Ok(Json(AuthStatusResponse {
        setup_required,
        authenticated: session.is_some(),
        user: session.as_ref().map(|s| s.to_user_info()),
        impersonating: session.and_then(|s| s.impersonating),
        all_permissions: all_perms,
    }))
}

async fn setup_admin_handler(
    State(state): State<AppState>,
    Json(payload): Json<SetupBody>,
) -> Result<Json<AuthResponse>, (StatusCode, String)> {
    let setup_required = state
        .user_mgr
        .is_setup_required()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if !setup_required {
        return Err((
            StatusCode::BAD_REQUEST,
            "Setup has already been completed".to_string(),
        ));
    }

    let user_info = state
        .user_mgr
        .setup_initial_admin(&payload.username, &payload.password)
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;

    let session = state.session_mgr.create_session(&user_info);

    state.log_buffer.push(
        "heimdall",
        "INFO",
        &format!(
            "Initial administrator account '{}' registered",
            user_info.nickname
        ),
    );

    Ok(Json(AuthResponse {
        token: session.token,
        user: user_info,
    }))
}

async fn login_handler(
    State(state): State<AppState>,
    Json(payload): Json<LoginBody>,
) -> Result<Json<AuthResponse>, (StatusCode, String)> {
    let user_info = state
        .user_mgr
        .authenticate(&payload.username, &payload.password)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or((
            StatusCode::UNAUTHORIZED,
            "Invalid nickname or password".to_string(),
        ))?;

    if !user_info.is_admin
        && !user_info
            .permissions
            .iter()
            .any(|p| p == PERM_HEIMDALL_LOGIN || p == PERM_ADMIN)
    {
        return Err((
            StatusCode::FORBIDDEN,
            "Access Denied: You do not have permission to log into Heimdall".to_string(),
        ));
    }

    let session = state.session_mgr.create_session(&user_info);

    state.log_buffer.push(
        "heimdall",
        "INFO",
        &format!("User '{}' logged into Heimdall", user_info.nickname),
    );

    Ok(Json(AuthResponse {
        token: session.token,
        user: user_info,
    }))
}

async fn logout_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<StatusCode, (StatusCode, String)> {
    if let Some(tok) = state.session_mgr.extract_token(&headers, None) {
        state.session_mgr.destroy_session(&tok);
    }
    Ok(StatusCode::OK)
}

async fn get_me_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Session>, (StatusCode, String)> {
    let session = require_auth_session(&state, &headers)?;
    Ok(Json(session))
}

async fn change_my_password_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<ChangePasswordBody>,
) -> Result<StatusCode, (StatusCode, String)> {
    let session = require_auth_session(&state, &headers)?;
    state
        .user_mgr
        .change_password(
            &session.user_id_hex,
            &payload.old_password,
            &payload.new_password,
        )
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    Ok(StatusCode::OK)
}

async fn stop_impersonating_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Session>, (StatusCode, String)> {
    let session = require_auth_session(&state, &headers)?;
    let imp = session.impersonating.ok_or((
        StatusCode::BAD_REQUEST,
        "Not currently impersonating".to_string(),
    ))?;
    let admin_user = state
        .user_mgr
        .get_user(&imp.admin_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or((
            StatusCode::NOT_FOUND,
            "Original admin user not found".to_string(),
        ))?;

    let restored = state
        .session_mgr
        .stop_impersonating(&session.token, &admin_user)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    state.log_buffer.push(
        "heimdall",
        "INFO",
        &format!(
            "Admin '{}' returned from impersonation",
            admin_user.nickname
        ),
    );

    Ok(Json(restored))
}

async fn list_users_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<UserInfo>>, (StatusCode, String)> {
    require_perm(&state, &headers, PERM_HEIMDALL_USERS)?;
    state
        .user_mgr
        .list_users()
        .map(Json)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

async fn create_user_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<CreateUserBody>,
) -> Result<Json<UserInfo>, (StatusCode, String)> {
    require_perm(&state, &headers, PERM_HEIMDALL_USERS)?;
    let perms = payload.permissions.unwrap_or_default();
    state
        .user_mgr
        .create_user(&payload.username, &payload.password, perms)
        .map(Json)
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))
}

async fn get_user_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    AxumPath(node_id): AxumPath<String>,
) -> Result<Json<UserInfo>, (StatusCode, String)> {
    require_perm(&state, &headers, PERM_HEIMDALL_USERS)?;
    state
        .user_mgr
        .get_user(&node_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .map(Json)
        .ok_or((StatusCode::NOT_FOUND, "User not found".to_string()))
}

async fn update_user_permissions_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    AxumPath(node_id): AxumPath<String>,
    Json(payload): Json<UpdatePermissionsBody>,
) -> Result<StatusCode, (StatusCode, String)> {
    require_perm(&state, &headers, PERM_HEIMDALL_USERS)?;
    state
        .user_mgr
        .update_permissions(&node_id, payload.permissions)
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    Ok(StatusCode::OK)
}

async fn reset_user_password_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    AxumPath(node_id): AxumPath<String>,
    Json(payload): Json<ResetPasswordBody>,
) -> Result<StatusCode, (StatusCode, String)> {
    require_perm(&state, &headers, PERM_HEIMDALL_USERS)?;
    state
        .user_mgr
        .reset_password(&node_id, &payload.new_password)
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    Ok(StatusCode::OK)
}

async fn impersonate_user_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    AxumPath(node_id): AxumPath<String>,
) -> Result<Json<Session>, (StatusCode, String)> {
    let session = require_perm(&state, &headers, PERM_HEIMDALL_USERS)?;
    let target_user = state
        .user_mgr
        .get_user(&node_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or((StatusCode::NOT_FOUND, "Target user not found".to_string()))?;

    let updated = state
        .session_mgr
        .impersonate(&session.token, &target_user)
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;

    state.log_buffer.push(
        "heimdall",
        "INFO",
        &format!(
            "Admin '{}' is now impersonating '{}'",
            session.username, target_user.nickname
        ),
    );

    Ok(Json(updated))
}

async fn delete_user_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    AxumPath(node_id): AxumPath<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    require_perm(&state, &headers, PERM_HEIMDALL_USERS)?;
    state
        .user_mgr
        .delete_user(&node_id)
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    Ok(StatusCode::OK)
}

// --- STATIC FILE SERVING ---

async fn serve_index_html(State(state): State<AppState>) -> impl IntoResponse {
    if let Some(ref dir) = state.web_dir {
        let p = dir.join("index.html");
        if p.exists() {
            if let Ok(c) = std::fs::read_to_string(p) {
                return (
                    [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
                    Html(c),
                )
                    .into_response();
            }
        }
    }
    (
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        Html(EMBEDDED_INDEX_HTML),
    )
        .into_response()
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
    (
        [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
        EMBEDDED_STYLE_CSS,
    )
        .into_response()
}

async fn serve_app_js(State(state): State<AppState>) -> impl IntoResponse {
    if let Some(ref dir) = state.web_dir {
        let p = dir.join("app.js");
        if p.exists() {
            if let Ok(c) = std::fs::read_to_string(p) {
                return (
                    [(
                        header::CONTENT_TYPE,
                        "application/javascript; charset=utf-8",
                    )],
                    c,
                )
                    .into_response();
            }
        }
    }
    (
        [(
            header::CONTENT_TYPE,
            "application/javascript; charset=utf-8",
        )],
        EMBEDDED_APP_JS,
    )
        .into_response()
}

// --- REST HANDLERS ---

async fn get_supervisor_status(State(state): State<AppState>) -> impl IntoResponse {
    let list = state.supervisor.get_all_status().await;
    Json(list)
}

async fn start_bbs_handler(
    State(state): State<AppState>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let cfg_path = state
        .config_mgr
        .get_config_path()
        .to_string_lossy()
        .to_string();
    state
        .supervisor
        .start_bbs(Some(&cfg_path), Some("captured_packets"))
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(serde_json::json!({ "status": "started" })))
}

async fn stop_bbs_handler(
    State(state): State<AppState>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    state
        .supervisor
        .stop_bbs()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(serde_json::json!({ "status": "stopped" })))
}

async fn restart_bbs_handler(
    State(state): State<AppState>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let cfg_path = state
        .config_mgr
        .get_config_path()
        .to_string_lossy()
        .to_string();
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
    Ok(Json(
        serde_json::json!({ "status": "crawler_started", "steps": steps }),
    ))
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
    Ok(Json(
        serde_json::json!({ "status": "tuning_started", "command": q.command }),
    ))
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

async fn get_config_handler(
    State(state): State<AppState>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
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
    let apps = state
        .app_mgr
        .list_apps(&cfg.apps.enabled, &cfg.apps.main_app);
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
    Ok(Json(
        serde_json::json!({ "status": "saved", "path": q.path }),
    ))
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

async fn toggle_app_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    AxumPath(app_id): AxumPath<String>,
    Json(body): Json<ToggleAppBody>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    require_perm(&state, &headers, PERM_ADMIN)?;

    let mut cfg = state.config_mgr.get_config();
    if body.enabled {
        if !cfg.apps.enabled.iter().any(|e| e == &app_id) {
            cfg.apps.enabled.push(app_id.clone());
        }
    } else {
        cfg.apps.enabled.retain(|e| e != &app_id);
    }
    let all_enabled = cfg.apps.enabled.clone();
    state
        .config_mgr
        .save_config(cfg)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(serde_json::json!({
        "app_id": app_id,
        "enabled": body.enabled,
        "all_enabled": all_enabled,
    })))
}

async fn get_catalog_handler(
    State(state): State<AppState>,
    Query(q): Query<CatalogQuery>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let force_refresh = q.refresh.unwrap_or(false);
    let catalog = state
        .app_mgr
        .fetch_catalog(None, force_refresh)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let cfg = state.config_mgr.get_config();
    let statuses =
        state
            .app_mgr
            .get_catalog_status(&catalog, &cfg.apps.enabled, &cfg.apps.main_app);
    Ok(Json(serde_json::json!({
        "catalog_version": catalog.catalog_version,
        "updated_at": catalog.updated_at,
        "apps": statuses,
    })))
}

async fn install_catalog_app_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<InstallCatalogAppBody>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    require_perm(&state, &headers, PERM_ADMIN)?;

    let catalog = state
        .app_mgr
        .fetch_catalog(None, false)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    state
        .app_mgr
        .install_catalog_app(&body.app_id, body.tag.as_deref(), &catalog)
        .await
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;

    // Automatically enable in config.toml if not already enabled
    let mut cfg = state.config_mgr.get_config();
    if !cfg.apps.enabled.iter().any(|e| e == &body.app_id) {
        cfg.apps.enabled.push(body.app_id.clone());
        let _ = state.config_mgr.save_config(cfg);
    }

    Ok(Json(serde_json::json!({
        "status": "installed",
        "app_id": body.app_id,
        "tag": body.tag,
    })))
}

async fn uninstall_catalog_app_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<UninstallCatalogAppBody>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    require_perm(&state, &headers, PERM_ADMIN)?;

    state
        .app_mgr
        .uninstall_app(&body.app_id)
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;

    // Disable in config.toml
    let mut cfg = state.config_mgr.get_config();
    cfg.apps.enabled.retain(|e| e != &body.app_id);
    let _ = state.config_mgr.save_config(cfg);

    Ok(Json(serde_json::json!({
        "status": "uninstalled",
        "app_id": body.app_id,
    })))
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
    headers.insert(
        header::CONTENT_TYPE,
        "application/octet-stream".parse().unwrap(),
    );
    headers.insert(
        header::CONTENT_DISPOSITION,
        format!("attachment; filename=\"{}\"", filename)
            .parse()
            .unwrap(),
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
    Ok(Json(
        serde_json::json!({ "status": "ok", "restored": true, "bytes": body.len() }),
    ))
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
    Ok(Json(
        serde_json::json!({ "status": "ok", "cleared_records": count }),
    ))
}

async fn get_database_key_handler(
    State(state): State<AppState>,
    AxumPath((namespace, key)): AxumPath<(String, String)>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let val = state
        .db_mgr
        .get_key(&namespace, &key)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(
        serde_json::json!({ "namespace": namespace, "key": key, "value": val }),
    ))
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
    Ok(Json(
        serde_json::json!({ "status": "ok", "namespace": namespace, "key": key }),
    ))
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

// --- MULTI-BBS NETWORK REGISTRY HANDLERS ---

async fn get_network_registry_handler(
    State(state): State<AppState>,
    Query(q): Query<NetworkQuery>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let cfg = state.config_mgr.get_config();
    let cache_path = bifrost_bbs::find_workspace_path(&cfg.network.registry_cache_file);
    let reg_mgr = BbsNetworkRegistryManager::new(cache_path);

    if q.refresh.unwrap_or(false) {
        let _ = reg_mgr.sync_from_url(&cfg.network.registry_url);
    }

    let nodes = if let Some(query_str) = q.q {
        reg_mgr.search(&query_str)
    } else {
        reg_mgr.get_nodes()
    };

    Ok(Json(serde_json::json!({
        "network_enabled": cfg.network.enabled,
        "max_hops": cfg.network.max_hops,
        "allow_inbound_relay": cfg.network.allow_inbound_relay,
        "registry_url": cfg.network.registry_url,
        "total_nodes": nodes.len(),
        "nodes": nodes,
    })))
}

async fn sync_network_registry_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    require_perm(&state, &headers, PERM_ADMIN)?;
    let cfg = state.config_mgr.get_config();
    let cache_path = bifrost_bbs::find_workspace_path(&cfg.network.registry_cache_file);
    let reg_mgr = BbsNetworkRegistryManager::new(cache_path);

    let count = reg_mgr
        .sync_from_url(&cfg.network.registry_url)
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Sync failed: {}", e),
            )
        })?;

    Ok(Json(serde_json::json!({
        "success": true,
        "synced_nodes": count,
    })))
}

async fn test_network_ping_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<TestPingBody>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    require_perm(&state, &headers, PERM_ADMIN)?;

    let addr = format!("{}:{}", body.host, body.port);
    let start = std::time::Instant::now();
    let timeout = std::time::Duration::from_millis(1500);

    let result = tokio::time::timeout(timeout, tokio::net::TcpStream::connect(&addr)).await;
    match result {
        Ok(Ok(_)) => {
            let latency_ms = start.elapsed().as_millis() as u64;
            Ok(Json(serde_json::json!({
                "reachable": true,
                "latency_ms": latency_ms,
                "endpoint": addr,
            })))
        }
        Ok(Err(e)) => Ok(Json(serde_json::json!({
            "reachable": false,
            "error": e.to_string(),
            "endpoint": addr,
        }))),
        Err(_) => Ok(Json(serde_json::json!({
            "reachable": false,
            "error": "Connection timed out (1500ms)".to_string(),
            "endpoint": addr,
        }))),
    }
}

// --- WEBSOCKET HANDLERS ---

async fn ws_logs_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
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
    headers: HeaderMap,
    Query(tok_query): Query<TokenQuery>,
    State(state): State<AppState>,
) -> Response {
    let session = extract_session(&state, &headers, tok_query.token.as_deref());
    let authenticated_node = session.map(|s| s.user_id);
    let port = state.radio_port.clone();
    let log_buf = state.log_buffer.clone();
    ws.on_upgrade(move |socket| handle_web_terminal_ws(socket, port, log_buf, authenticated_node))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    #[tokio::test]
    async fn test_web_routes_and_static_fallback() {
        let temp_dir = std::env::temp_dir().join(format!(
            "heimdall_web_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&temp_dir).unwrap();

        let log_buf = Arc::new(LogBuffer::default());
        let supervisor = Supervisor::new(&temp_dir, log_buf.clone());
        let config_mgr = Arc::new(ConfigManager::new(temp_dir.join("config.toml")));
        let app_mgr = Arc::new(AppManager::new(temp_dir.join("apps")));
        let stats_mgr = Arc::new(StatsManager::new(temp_dir.join("captures")));
        let db_path = temp_dir.join("database.db");
        let db_mgr = Arc::new(DatabaseManager::new(&db_path));
        let user_mgr = Arc::new(UserManager::new(&db_path));
        let session_mgr = Arc::new(SessionManager::new());

        let state = AppState {
            supervisor,
            config_mgr,
            app_mgr,
            stats_mgr,
            db_mgr,
            user_mgr,
            session_mgr,
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
        let req_css = Request::builder()
            .uri("/style.css")
            .body(Body::empty())
            .unwrap();
        let resp_css = app.clone().oneshot(req_css).await.unwrap();
        assert_eq!(resp_css.status(), StatusCode::OK);

        // Test GET /api/auth/status
        let req_auth_stat = Request::builder()
            .uri("/api/auth/status")
            .body(Body::empty())
            .unwrap();
        let resp_auth_stat = app.clone().oneshot(req_auth_stat).await.unwrap();
        assert_eq!(resp_auth_stat.status(), StatusCode::OK);

        // Test POST /api/auth/setup
        let setup_body = serde_json::json!({
            "username": "AdminUser",
            "password": "AdminPassword123"
        })
        .to_string();
        let req_setup = Request::builder()
            .method("POST")
            .uri("/api/auth/setup")
            .header("Content-Type", "application/json")
            .body(Body::from(setup_body))
            .unwrap();
        let resp_setup = app.clone().oneshot(req_setup).await.unwrap();
        assert_eq!(resp_setup.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(resp_setup.into_body(), usize::MAX)
            .await
            .unwrap();
        let auth_res: AuthResponse = serde_json::from_slice(&body_bytes).unwrap();
        let token = auth_res.token;

        // Test GET /api/supervisor/status
        let req_status = Request::builder()
            .uri("/api/supervisor/status")
            .body(Body::empty())
            .unwrap();
        let resp_status = app.clone().oneshot(req_status).await.unwrap();
        assert_eq!(resp_status.status(), StatusCode::OK);

        // Test GET /api/users
        let req_users = Request::builder()
            .uri("/api/users")
            .header("Authorization", format!("Bearer {}", token))
            .body(Body::empty())
            .unwrap();
        let resp_users = app.clone().oneshot(req_users).await.unwrap();
        assert_eq!(resp_users.status(), StatusCode::OK);

        // Test POST /api/users
        let create_user_body = serde_json::json!({
            "username": "BobUser",
            "password": "BobPassword123",
            "permissions": [PERM_HEIMDALL_LOGIN, "heimdall.terminal"]
        })
        .to_string();
        let req_create_u = Request::builder()
            .method("POST")
            .uri("/api/users")
            .header("Authorization", format!("Bearer {}", token))
            .header("Content-Type", "application/json")
            .body(Body::from(create_user_body))
            .unwrap();
        let resp_create_u = app.clone().oneshot(req_create_u).await.unwrap();
        assert_eq!(resp_create_u.status(), StatusCode::OK);

        // Test GET /api/catalog
        let req_catalog = Request::builder()
            .uri("/api/catalog")
            .body(Body::empty())
            .unwrap();
        let resp_catalog = app.clone().oneshot(req_catalog).await.unwrap();
        assert_eq!(resp_catalog.status(), StatusCode::OK);
        let cat_bytes = axum::body::to_bytes(resp_catalog.into_body(), usize::MAX)
            .await
            .unwrap();
        let cat_val: serde_json::Value = serde_json::from_slice(&cat_bytes).unwrap();
        assert!(cat_val["apps"].as_array().unwrap().len() >= 4);

        // Test GET /api/network
        let req_network = Request::builder()
            .uri("/api/network")
            .body(Body::empty())
            .unwrap();
        let resp_network = app.clone().oneshot(req_network).await.unwrap();
        assert_eq!(resp_network.status(), StatusCode::OK);
        let net_bytes = axum::body::to_bytes(resp_network.into_body(), usize::MAX)
            .await
            .unwrap();
        let net_val: serde_json::Value = serde_json::from_slice(&net_bytes).unwrap();
        assert_eq!(net_val["network_enabled"], true);
        assert!(net_val["nodes"].as_array().unwrap().len() >= 3);

        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
