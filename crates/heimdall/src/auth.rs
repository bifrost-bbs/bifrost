//! Authentication and session management supporting token authentication,
//! granular permissions, and admin impersonation.

use crate::user_mgr::{hex_to_node_id, UserInfo, PERM_ADMIN};
use axum::http::HeaderMap;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AuthConfig {
    pub enabled: bool,
    pub api_key: Option<String>,
    pub auth_token: Option<String>,
    pub username: Option<String>,
    pub password: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImpersonationInfo {
    pub admin_id: String,
    pub admin_username: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub token: String,
    pub user_id: [u8; 32],
    pub user_id_hex: String,
    pub username: String,
    pub permissions: Vec<String>,
    pub is_admin: bool,
    pub impersonating: Option<ImpersonationInfo>,
    #[serde(skip)]
    pub created_instant: Option<Instant>,
    pub created_at: u64,
}

impl Session {
    pub fn has_permission(&self, perm: &str) -> bool {
        if self.is_admin || self.permissions.iter().any(|p| p == PERM_ADMIN || p == "*") {
            return true;
        }
        self.permissions.iter().any(|p| p == perm)
    }

    pub fn to_user_info(&self) -> UserInfo {
        UserInfo {
            id: self.user_id_hex.clone(),
            nickname: self.username.clone(),
            has_password: true,
            permissions: self.permissions.clone(),
            is_admin: self.is_admin,
            created_at: self.created_at,
            updated_at: self.created_at,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct SessionManager {
    sessions: Arc<Mutex<HashMap<String, Session>>>,
}

impl SessionManager {
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn create_session(&self, user: &UserInfo) -> Session {
        let mut rand_bytes = [0u8; 24];
        for b in rand_bytes.iter_mut() {
            *b = rand_byte();
        }
        let token = format!("hmd_{}", hex_encode(&rand_bytes));
        let user_id = hex_to_node_id(&user.id).unwrap_or([0u8; 32]);
        let now_sec = current_unix_timestamp();

        let session = Session {
            token: token.clone(),
            user_id,
            user_id_hex: user.id.clone(),
            username: user.nickname.clone(),
            permissions: user.permissions.clone(),
            is_admin: user.is_admin,
            impersonating: None,
            created_instant: Some(Instant::now()),
            created_at: now_sec,
        };

        let mut lock = self.sessions.lock().unwrap();
        lock.insert(token, session.clone());
        session
    }

    pub fn get_session(&self, token: &str) -> Option<Session> {
        let lock = self.sessions.lock().unwrap();
        lock.get(token).cloned()
    }

    pub fn destroy_session(&self, token: &str) -> bool {
        let mut lock = self.sessions.lock().unwrap();
        lock.remove(token).is_some()
    }

    pub fn impersonate(&self, token: &str, target_user: &UserInfo) -> anyhow::Result<Session> {
        let mut lock = self.sessions.lock().unwrap();
        let current_session = match lock.get_mut(token) {
            Some(s) => s,
            None => anyhow::bail!("Session not found"),
        };

        if !current_session.is_admin && !current_session.has_permission(crate::user_mgr::PERM_HEIMDALL_USERS) {
            anyhow::bail!("Only administrators can impersonate users");
        }

        let admin_id = current_session.impersonating.as_ref().map(|i| i.admin_id.clone())
            .unwrap_or_else(|| current_session.user_id_hex.clone());
        let admin_username = current_session.impersonating.as_ref().map(|i| i.admin_username.clone())
            .unwrap_or_else(|| current_session.username.clone());

        let target_id = hex_to_node_id(&target_user.id).unwrap_or([0u8; 32]);

        current_session.user_id = target_id;
        current_session.user_id_hex = target_user.id.clone();
        current_session.username = target_user.nickname.clone();
        current_session.permissions = target_user.permissions.clone();
        current_session.is_admin = target_user.is_admin;
        current_session.impersonating = Some(ImpersonationInfo {
            admin_id,
            admin_username,
        });

        Ok(current_session.clone())
    }

    pub fn stop_impersonating(&self, token: &str, original_admin_user: &UserInfo) -> anyhow::Result<Session> {
        let mut lock = self.sessions.lock().unwrap();
        let current_session = match lock.get_mut(token) {
            Some(s) => s,
            None => anyhow::bail!("Session not found"),
        };

        if current_session.impersonating.is_none() {
            return Ok(current_session.clone());
        }

        let admin_id = hex_to_node_id(&original_admin_user.id).unwrap_or([0u8; 32]);
        current_session.user_id = admin_id;
        current_session.user_id_hex = original_admin_user.id.clone();
        current_session.username = original_admin_user.nickname.clone();
        current_session.permissions = original_admin_user.permissions.clone();
        current_session.is_admin = original_admin_user.is_admin;
        current_session.impersonating = None;

        Ok(current_session.clone())
    }

    pub fn extract_token(&self, headers: &HeaderMap, query_token: Option<&str>) -> Option<String> {
        // 1. Authorization: Bearer <token>
        if let Some(auth_hdr) = headers.get("Authorization").and_then(|h| h.to_str().ok()) {
            if auth_hdr.starts_with("Bearer ") {
                let tok = auth_hdr[7..].trim();
                if !tok.is_empty() {
                    return Some(tok.to_string());
                }
            }
        }

        // 2. x-auth-token: <token>
        if let Some(tok_hdr) = headers.get("x-auth-token").and_then(|h| h.to_str().ok()) {
            let tok = tok_hdr.trim();
            if !tok.is_empty() {
                return Some(tok.to_string());
            }
        }

        // 3. Query string ?token=<token>
        if let Some(tok) = query_token {
            let tok = tok.trim();
            if !tok.is_empty() {
                return Some(tok.to_string());
            }
        }

        None
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct TokenQuery {
    pub token: Option<String>,
}

fn rand_byte() -> u8 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(42);
    let ptr = Box::into_raw(Box::new(nanos)) as usize;
    ((nanos ^ (nanos >> 7) ^ (ptr as u32)) & 0xFF) as u8
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

fn current_unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::user_mgr::PERM_HEIMDALL_LOGIN;

    #[test]
    fn test_session_creation_and_permission() {
        let mgr = SessionManager::new();
        let user = UserInfo {
            id: "0102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f20".to_string(),
            nickname: "TestAdmin".to_string(),
            has_password: true,
            permissions: vec![PERM_ADMIN.to_string(), PERM_HEIMDALL_LOGIN.to_string()],
            is_admin: true,
            created_at: 1000,
            updated_at: 1000,
        };

        let session = mgr.create_session(&user);
        assert!(session.token.starts_with("hmd_"));
        assert!(session.has_permission(PERM_ADMIN));
        assert!(session.has_permission(PERM_HEIMDALL_LOGIN));
        assert!(session.has_permission("any.random.perm")); // Admin has all permissions

        let fetched = mgr.get_session(&session.token).unwrap();
        assert_eq!(fetched.username, "TestAdmin");

        // Non-admin user
        let user2 = UserInfo {
            id: "2102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f20".to_string(),
            nickname: "UserBob".to_string(),
            has_password: true,
            permissions: vec![PERM_HEIMDALL_LOGIN.to_string(), "heimdall.terminal".to_string()],
            is_admin: false,
            created_at: 1000,
            updated_at: 1000,
        };
        let s2 = mgr.create_session(&user2);
        assert!(s2.has_permission(PERM_HEIMDALL_LOGIN));
        assert!(s2.has_permission("heimdall.terminal"));
        assert!(!s2.has_permission("heimdall.users"));
        assert!(!s2.has_permission(PERM_ADMIN));

        // Impersonation
        let imp_session = mgr.impersonate(&session.token, &user2).unwrap();
        assert_eq!(imp_session.username, "UserBob");
        assert_eq!(imp_session.impersonating.as_ref().unwrap().admin_username, "TestAdmin");
        assert!(!imp_session.has_permission("heimdall.users"));

        // Stop impersonation
        let restored = mgr.stop_impersonating(&session.token, &user).unwrap();
        assert_eq!(restored.username, "TestAdmin");
        assert!(restored.impersonating.is_none());
        assert!(restored.has_permission("heimdall.users"));
    }
}
