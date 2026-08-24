//! User management, password hashing, and permissions system for Heimdall and MeshBBS.

use anyhow::{bail, Context, Result};
use bifrost_bbs::DatabaseStore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Standard permissions recognized across Heimdall and Bifrost BBS.
pub const PERM_ADMIN: &str = "admin";
pub const PERM_READ: &str = "read";
pub const PERM_WRITE: &str = "write";

pub const PERM_HEIMDALL_LOGIN: &str = "heimdall.login";
pub const PERM_HEIMDALL_OVERVIEW: &str = "heimdall.overview";
pub const PERM_HEIMDALL_TERMINAL: &str = "heimdall.terminal";
pub const PERM_HEIMDALL_LOGS: &str = "heimdall.logs";
pub const PERM_HEIMDALL_APPS: &str = "heimdall.apps";
pub const PERM_HEIMDALL_CONFIG: &str = "heimdall.config";
pub const PERM_HEIMDALL_TELEMETRY: &str = "heimdall.telemetry";
pub const PERM_HEIMDALL_TUNING: &str = "heimdall.tuning";
pub const PERM_HEIMDALL_DATABASE: &str = "heimdall.database";
pub const PERM_HEIMDALL_USERS: &str = "heimdall.users";
pub const PERM_HEIMDALL_SUPERVISOR: &str = "heimdall.supervisor";

/// List of all available Heimdall permissions for UI assignment.
pub const ALL_HEIMDALL_PERMISSIONS: &[(&str, &str, &str)] = &[
    (
        PERM_HEIMDALL_LOGIN,
        "Login Access",
        "Allows authenticating into Heimdall Web UI and API",
    ),
    (
        PERM_HEIMDALL_OVERVIEW,
        "Overview Dashboard",
        "View system overview and health cards",
    ),
    (
        PERM_HEIMDALL_TERMINAL,
        "Web Terminal",
        "Access the interactive CP437 ANSI Web Terminal",
    ),
    (
        PERM_HEIMDALL_LOGS,
        "System Logs",
        "View and stream realtime supervisor and BBS logs",
    ),
    (
        PERM_HEIMDALL_APPS,
        "App Catalog & Editor",
        "View, test, and edit Lua BBS applications",
    ),
    (
        PERM_HEIMDALL_CONFIG,
        "Configuration",
        "View and modify supervisor and BBS settings",
    ),
    (
        PERM_HEIMDALL_TELEMETRY,
        "Telemetry & Captures",
        "Inspect airtime, packet telemetry, and captures",
    ),
    (
        PERM_HEIMDALL_TUNING,
        "Tuning & Crawler",
        "Execute compression training and automated crawlers",
    ),
    (
        PERM_HEIMDALL_DATABASE,
        "Database Management",
        "Inspect, query, backup, restore, and reset SQLite DB",
    ),
    (
        PERM_HEIMDALL_USERS,
        "User & Role Management",
        "Manage accounts, permissions, password resets, and impersonation",
    ),
    (
        PERM_HEIMDALL_SUPERVISOR,
        "Supervisor Commands",
        "Start, stop, and restart BBS and crawler services",
    ),
];

/// User record as stored in the `users` SQLite namespace.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserRecord {
    pub nickname: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password_hash: Option<String>,
    #[serde(default)]
    pub created_at: u64,
    #[serde(default)]
    pub updated_at: u64,
}

/// User details exposed to clients and API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserInfo {
    pub id: String, // 64-char hex node ID
    pub nickname: String,
    pub has_password: bool,
    pub permissions: Vec<String>,
    pub is_admin: bool,
    pub created_at: u64,
    pub updated_at: u64,
}

#[derive(Debug, Clone)]
pub struct UserManager {
    db_path: PathBuf,
}

impl UserManager {
    pub fn new(db_path: impl AsRef<Path>) -> Self {
        Self {
            db_path: db_path.as_ref().to_path_buf(),
        }
    }

    fn get_store(&self) -> Result<DatabaseStore> {
        if let Some(parent) = self.db_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        DatabaseStore::new(&self.db_path)
            .with_context(|| format!("Failed to open database at {:?}", self.db_path))
    }

    /// Helper to parse user data in various formats (JSON struct, JSON object, plain string).
    fn parse_user_record(&self, raw: &str) -> (String, Option<String>, u64, u64) {
        if let Ok(rec) = serde_json::from_str::<UserRecord>(raw) {
            return (
                rec.nickname,
                rec.password_hash,
                rec.created_at,
                rec.updated_at,
            );
        }
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(raw) {
            if let Some(obj) = val.as_object() {
                let nick = obj
                    .get("nickname")
                    .or_else(|| obj.get("username"))
                    .or_else(|| obj.get("callsign"))
                    .or_else(|| obj.get("name"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("Unknown")
                    .to_string();
                let pass = obj
                    .get("password_hash")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let created = obj.get("created_at").and_then(|v| v.as_u64()).unwrap_or(0);
                let updated = obj.get("updated_at").and_then(|v| v.as_u64()).unwrap_or(0);
                return (nick, pass, created, updated);
            } else if let Some(s) = val.as_str() {
                return (s.to_string(), None, 0, 0);
            }
        }
        let trimmed = raw.trim().trim_matches('"');
        if !trimmed.is_empty() {
            (trimmed.to_string(), None, 0, 0)
        } else {
            ("Unknown".to_string(), None, 0, 0)
        }
    }

    /// Check if initial setup is required (i.e. no users exist with a password or no admin exists).
    pub fn is_setup_required(&self) -> Result<bool> {
        let store = self.get_store()?;
        let user_entries = store.get_all("users").unwrap_or_default();
        if user_entries.is_empty() {
            return Ok(true);
        }

        // Check if at least one user has a password and admin/login permission
        for (node_id_hex, json_str) in user_entries {
            let (_, password_hash, _, _) = self.parse_user_record(&json_str);
            if password_hash.is_some() {
                let perms = self.get_user_permissions_internal(&store, &node_id_hex);
                if perms
                    .iter()
                    .any(|p| p == PERM_ADMIN || p == PERM_HEIMDALL_LOGIN)
                {
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }

    /// Helper to get permissions for a node hex from `permissions` namespace.
    fn get_user_permissions_internal(
        &self,
        store: &DatabaseStore,
        node_id_hex: &str,
    ) -> Vec<String> {
        if let Ok(Some(json_str)) = store.get("permissions", node_id_hex) {
            if let Ok(perms) = serde_json::from_str::<Vec<String>>(&json_str) {
                return perms;
            }
        }
        Vec::new()
    }

    /// Setup the first administrator user with full permissions.
    pub fn setup_initial_admin(&self, username: &str, password: &str) -> Result<UserInfo> {
        let username = username.trim();
        if username.is_empty() {
            bail!("Username / nickname cannot be empty");
        }
        if password.len() < 4 {
            bail!("Password must be at least 4 characters long");
        }

        let store = self.get_store()?;

        // Check if a user with this nickname already exists
        let mut target_node_id = None;
        for (node_id_hex, json_str) in store.get_all("users").unwrap_or_default() {
            let (nick, _, _, _) = self.parse_user_record(&json_str);
            if nick.eq_ignore_ascii_case(username) {
                target_node_id = Some(node_id_hex);
                break;
            }
        }

        let now = current_unix_timestamp();
        let node_id_hex = target_node_id.unwrap_or_else(|| {
            let mut rand_node = [0u8; 32];
            for b in rand_node.iter_mut() {
                *b = rand_byte();
            }
            hex_encode(&rand_node)
        });

        let password_hash = hash_password(password);
        let user_rec = UserRecord {
            nickname: username.to_string(),
            password_hash: Some(password_hash),
            created_at: now,
            updated_at: now,
        };

        store.set("users", &node_id_hex, &serde_json::to_string(&user_rec)?)?;

        // Grant full admin + all heimdall permissions
        let mut admin_perms = vec![
            PERM_ADMIN.to_string(),
            PERM_READ.to_string(),
            PERM_WRITE.to_string(),
        ];
        for (perm, _, _) in ALL_HEIMDALL_PERMISSIONS {
            if !admin_perms.contains(&perm.to_string()) {
                admin_perms.push(perm.to_string());
            }
        }

        store.set(
            "permissions",
            &node_id_hex,
            &serde_json::to_string(&admin_perms)?,
        )?;

        Ok(UserInfo {
            id: node_id_hex,
            nickname: username.to_string(),
            has_password: true,
            permissions: admin_perms,
            is_admin: true,
            created_at: now,
            updated_at: now,
        })
    }

    /// Authenticate a user by username (nickname) and password.
    pub fn authenticate(&self, username: &str, password: &str) -> Result<Option<UserInfo>> {
        let username = username.trim();
        if username.is_empty() || password.is_empty() {
            return Ok(None);
        }

        let store = self.get_store()?;
        for (node_id_hex, json_str) in store.get_all("users").unwrap_or_default() {
            let (nick, stored_hash_opt, created_at, updated_at) = self.parse_user_record(&json_str);
            if nick.eq_ignore_ascii_case(username) {
                if let Some(ref stored_hash) = stored_hash_opt {
                    if verify_password(password, stored_hash) {
                        let permissions = self.get_user_permissions_internal(&store, &node_id_hex);
                        let is_admin = permissions.iter().any(|p| p == PERM_ADMIN || p == "*");
                        return Ok(Some(UserInfo {
                            id: node_id_hex,
                            nickname: nick,
                            has_password: true,
                            permissions,
                            is_admin,
                            created_at,
                            updated_at,
                        }));
                    }
                }
                // User found but password didn't match or not set
                return Ok(None);
            }
        }
        Ok(None)
    }

    /// List all registered users.
    pub fn list_users(&self) -> Result<Vec<UserInfo>> {
        let store = self.get_store()?;
        let mut users = Vec::new();

        for (node_id_hex, json_str) in store.get_all("users").unwrap_or_default() {
            let (nick, password_hash, created_at, updated_at) = self.parse_user_record(&json_str);
            let permissions = self.get_user_permissions_internal(&store, &node_id_hex);
            let is_admin = permissions.iter().any(|p| p == PERM_ADMIN || p == "*");
            users.push(UserInfo {
                id: node_id_hex,
                nickname: nick,
                has_password: password_hash.is_some(),
                permissions,
                is_admin,
                created_at,
                updated_at,
            });
        }

        users.sort_by(|a, b| a.nickname.to_lowercase().cmp(&b.nickname.to_lowercase()));
        Ok(users)
    }

    /// Retrieve user by node ID hex.
    pub fn get_user(&self, node_id_hex: &str) -> Result<Option<UserInfo>> {
        let store = self.get_store()?;
        if let Some(json_str) = store.get("users", node_id_hex)? {
            let (nick, password_hash, created_at, updated_at) = self.parse_user_record(&json_str);
            let permissions = self.get_user_permissions_internal(&store, node_id_hex);
            let is_admin = permissions.iter().any(|p| p == PERM_ADMIN || p == "*");
            return Ok(Some(UserInfo {
                id: node_id_hex.to_string(),
                nickname: nick,
                has_password: password_hash.is_some(),
                permissions,
                is_admin,
                created_at,
                updated_at,
            }));
        }
        Ok(None)
    }

    /// Create a new user with nickname, password, and custom permissions.
    pub fn create_user(
        &self,
        username: &str,
        password: &str,
        permissions: Vec<String>,
    ) -> Result<UserInfo> {
        let username = username.trim();
        if username.is_empty() {
            bail!("Username cannot be empty");
        }
        if password.len() < 4 {
            bail!("Password must be at least 4 characters long");
        }

        let store = self.get_store()?;

        // Check if nickname already exists
        for (_, json_str) in store.get_all("users").unwrap_or_default() {
            if let Ok(rec) = serde_json::from_str::<UserRecord>(&json_str) {
                if rec.nickname.eq_ignore_ascii_case(username) {
                    bail!("A user with nickname '{}' already exists", username);
                }
            }
        }

        let mut rand_node = [0u8; 32];
        for b in rand_node.iter_mut() {
            *b = rand_byte();
        }
        let node_id_hex = hex_encode(&rand_node);

        let now = current_unix_timestamp();
        let password_hash = hash_password(password);
        let user_rec = UserRecord {
            nickname: username.to_string(),
            password_hash: Some(password_hash),
            created_at: now,
            updated_at: now,
        };

        store.set("users", &node_id_hex, &serde_json::to_string(&user_rec)?)?;

        // Standard permissions if none provided
        let mut final_perms = permissions;
        if final_perms.is_empty() {
            final_perms = vec![
                PERM_READ.to_string(),
                PERM_WRITE.to_string(),
                PERM_HEIMDALL_LOGIN.to_string(),
                PERM_HEIMDALL_OVERVIEW.to_string(),
                PERM_HEIMDALL_TERMINAL.to_string(),
            ];
        }

        store.set(
            "permissions",
            &node_id_hex,
            &serde_json::to_string(&final_perms)?,
        )?;

        let is_admin = final_perms.iter().any(|p| p == PERM_ADMIN || p == "*");

        Ok(UserInfo {
            id: node_id_hex,
            nickname: username.to_string(),
            has_password: true,
            permissions: final_perms,
            is_admin,
            created_at: now,
            updated_at: now,
        })
    }

    /// Update user permissions.
    pub fn update_permissions(&self, node_id_hex: &str, permissions: Vec<String>) -> Result<()> {
        let store = self.get_store()?;
        if store.get("users", node_id_hex)?.is_none() {
            bail!("User not found");
        }

        store.set(
            "permissions",
            node_id_hex,
            &serde_json::to_string(&permissions)?,
        )?;
        Ok(())
    }

    /// Reset another user's password (by admin).
    pub fn reset_password(&self, node_id_hex: &str, new_password: &str) -> Result<()> {
        if new_password.len() < 4 {
            bail!("Password must be at least 4 characters long");
        }

        let store = self.get_store()?;
        let json_str = match store.get("users", node_id_hex)? {
            Some(j) => j,
            None => bail!("User not found"),
        };

        let mut user_rec: UserRecord = serde_json::from_str(&json_str)?;
        user_rec.password_hash = Some(hash_password(new_password));
        user_rec.updated_at = current_unix_timestamp();

        store.set("users", node_id_hex, &serde_json::to_string(&user_rec)?)?;
        Ok(())
    }

    /// Change current user's own password with old password verification.
    pub fn change_password(
        &self,
        node_id_hex: &str,
        old_password: &str,
        new_password: &str,
    ) -> Result<()> {
        if new_password.len() < 4 {
            bail!("New password must be at least 4 characters long");
        }

        let store = self.get_store()?;
        let json_str = match store.get("users", node_id_hex)? {
            Some(j) => j,
            None => bail!("User not found"),
        };

        let mut user_rec: UserRecord = serde_json::from_str(&json_str)?;
        if let Some(ref stored_hash) = user_rec.password_hash {
            if !verify_password(old_password, stored_hash) {
                bail!("Current password is incorrect");
            }
        }

        user_rec.password_hash = Some(hash_password(new_password));
        user_rec.updated_at = current_unix_timestamp();

        store.set("users", node_id_hex, &serde_json::to_string(&user_rec)?)?;
        Ok(())
    }

    /// Delete user and their permissions.
    pub fn delete_user(&self, node_id_hex: &str) -> Result<()> {
        let store = self.get_store()?;

        // Count total admins to ensure we don't delete the last admin
        let mut admin_count = 0;
        for (id, _) in store.get_all("users").unwrap_or_default() {
            let perms = self.get_user_permissions_internal(&store, &id);
            if perms.iter().any(|p| p == PERM_ADMIN || p == "*") {
                admin_count += 1;
            }
        }

        let user_perms = self.get_user_permissions_internal(&store, node_id_hex);
        let is_target_admin = user_perms.iter().any(|p| p == PERM_ADMIN || p == "*");

        if is_target_admin && admin_count <= 1 {
            bail!("Cannot delete the only administrator account");
        }

        store.remove("users", node_id_hex)?;
        store.remove("permissions", node_id_hex)?;
        Ok(())
    }
}

/// Simple salt + SHA-256 password hash.
pub fn hash_password(password: &str) -> String {
    let mut salt = [0u8; 16];
    for b in salt.iter_mut() {
        *b = rand_byte();
    }
    let salt_hex = hex_encode(&salt);

    let mut hasher = Sha256::new();
    hasher.update(salt_hex.as_bytes());
    hasher.update(password.as_bytes());
    let hash_bytes = hasher.finalize();
    let hash_hex = hex_encode(&hash_bytes);

    format!("{}${}", salt_hex, hash_hex)
}

/// Verify password against `salt$hash` string.
pub fn verify_password(password: &str, stored: &str) -> bool {
    if let Some((salt_hex, hash_hex)) = stored.split_once('$') {
        let mut hasher = Sha256::new();
        hasher.update(salt_hex.as_bytes());
        hasher.update(password.as_bytes());
        let computed = hex_encode(&hasher.finalize());
        computed == hash_hex
    } else {
        false
    }
}

pub fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

pub fn hex_decode(hex_str: &str) -> Option<Vec<u8>> {
    if hex_str.len() % 2 != 0 {
        return None;
    }
    (0..hex_str.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex_str[i..i + 2], 16).ok())
        .collect()
}

pub fn hex_to_node_id(hex_str: &str) -> Option<[u8; 32]> {
    let bytes = hex_decode(hex_str)?;
    if bytes.len() == 32 {
        let mut node = [0u8; 32];
        node.copy_from_slice(&bytes);
        Some(node)
    } else {
        None
    }
}

fn rand_byte() -> u8 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(42);
    let ptr = Box::into_raw(Box::new(nanos)) as usize;
    ((nanos ^ (nanos >> 7) ^ (ptr as u32)) & 0xFF) as u8
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

    #[test]
    fn test_password_hashing_and_verification() {
        let password = "SecretPassword123!";
        let hash = hash_password(password);
        assert!(hash.contains('$'));
        assert!(verify_password(password, &hash));
        assert!(!verify_password("WrongPassword", &hash));
    }

    #[test]
    fn test_user_manager_crud() {
        let temp_dir = std::env::temp_dir().join(format!("heimdall_user_test_{}", rand_byte()));
        let db_path = temp_dir.join("test_users.db");
        let user_mgr = UserManager::new(&db_path);

        assert!(user_mgr.is_setup_required().unwrap());

        // Setup initial admin
        let admin = user_mgr
            .setup_initial_admin("Commander", "admin123")
            .unwrap();
        assert_eq!(admin.nickname, "Commander");
        assert!(admin.is_admin);
        assert!(admin.permissions.contains(&PERM_ADMIN.to_string()));
        assert!(admin.permissions.contains(&PERM_HEIMDALL_LOGIN.to_string()));

        assert!(!user_mgr.is_setup_required().unwrap());

        // Authenticate admin
        let auth_res = user_mgr.authenticate("commander", "admin123").unwrap();
        assert!(auth_res.is_some());
        let authed = auth_res.unwrap();
        assert_eq!(authed.nickname, "Commander");

        // Fail auth
        assert!(user_mgr
            .authenticate("commander", "wrong")
            .unwrap()
            .is_none());

        // Create second user
        let user2 = user_mgr
            .create_user(
                "TraderBob",
                "bobpass",
                vec![
                    PERM_HEIMDALL_LOGIN.to_string(),
                    PERM_HEIMDALL_TERMINAL.to_string(),
                ],
            )
            .unwrap();
        assert_eq!(user2.nickname, "TraderBob");
        assert!(!user2.is_admin);

        // List users
        let list = user_mgr.list_users().unwrap();
        assert_eq!(list.len(), 2);

        // Update permissions
        user_mgr
            .update_permissions(
                &user2.id,
                vec![
                    PERM_HEIMDALL_LOGIN.to_string(),
                    PERM_HEIMDALL_TERMINAL.to_string(),
                    PERM_HEIMDALL_OVERVIEW.to_string(),
                ],
            )
            .unwrap();
        let u2_updated = user_mgr.get_user(&user2.id).unwrap().unwrap();
        assert!(u2_updated
            .permissions
            .contains(&PERM_HEIMDALL_OVERVIEW.to_string()));

        // Change password
        user_mgr
            .change_password(&user2.id, "bobpass", "newbobpass")
            .unwrap();
        assert!(user_mgr
            .authenticate("TraderBob", "bobpass")
            .unwrap()
            .is_none());
        assert!(user_mgr
            .authenticate("TraderBob", "newbobpass")
            .unwrap()
            .is_some());

        // Delete user
        user_mgr.delete_user(&user2.id).unwrap();
        assert_eq!(user_mgr.list_users().unwrap().len(), 1);

        let _ = std::fs::remove_dir_all(temp_dir);
    }
}
