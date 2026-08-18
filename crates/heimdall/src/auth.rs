//! Authentication middleware supporting HTTP Basic Auth and Bearer token verification.

use axum::extract::Request;
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AuthConfig {
    pub enabled: bool,
    pub username: Option<String>,
    pub password: Option<String>,
    pub auth_token: Option<String>,
}

impl AuthConfig {
    pub fn is_authorized(&self, headers: &HeaderMap) -> bool {
        if !self.enabled {
            return true;
        }

        // 1. Check Bearer Token in Authorization header or x-auth-token header
        if let Some(ref token) = self.auth_token {
            if let Some(auth_hdr) = headers.get("Authorization").and_then(|h| h.to_str().ok()) {
                if auth_hdr.starts_with("Bearer ") {
                    let provided = &auth_hdr[7..].trim();
                    if provided == token {
                        return true;
                    }
                }
            }

            if let Some(custom_hdr) = headers.get("x-auth-token").and_then(|h| h.to_str().ok()) {
                if custom_hdr == token {
                    return true;
                }
            }
        }

        // 2. Check HTTP Basic Auth
        if let (Some(ref u), Some(ref p)) = (&self.username, &self.password) {
            if let Some(auth_hdr) = headers.get("Authorization").and_then(|h| h.to_str().ok()) {
                if auth_hdr.starts_with("Basic ") {
                    let encoded = &auth_hdr[6..].trim();
                    if let Ok(decoded_bytes) = simple_base64_decode(encoded) {
                        if let Ok(decoded_str) = String::from_utf8(decoded_bytes) {
                            if let Some((user, pass)) = decoded_str.split_once(':') {
                                if user == u && pass == p {
                                    return true;
                                }
                            }
                        }
                    }
                }
            }
        }

        false
    }
}

pub async fn auth_middleware(
    auth_config: axum::extract::State<AuthConfig>,
    req: Request,
    next: Next,
) -> Response {
    if auth_config.is_authorized(req.headers()) {
        next.run(req).await
    } else {
        (
            StatusCode::UNAUTHORIZED,
            [("WWW-Authenticate", "Basic realm=\"Heimdall Admin\"")],
            "Unauthorized: Authentication Required",
        )
            .into_response()
    }
}

fn simple_base64_decode(s: &str) -> Result<Vec<u8>, &'static str> {
    const B64_CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::new();
    let mut buf = 0u32;
    let mut bits = 0;

    for &b in s.as_bytes() {
        if b == b'=' || b.is_ascii_whitespace() {
            continue;
        }
        let val = B64_CHARS.iter().position(|&c| c == b).ok_or("Invalid base64 character")? as u32;
        buf = (buf << 6) | val;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_auth_disabled() {
        let auth = AuthConfig {
            enabled: false,
            ..Default::default()
        };
        let headers = HeaderMap::new();
        assert!(auth.is_authorized(&headers));
    }

    #[test]
    fn test_auth_bearer_token() {
        let auth = AuthConfig {
            enabled: true,
            auth_token: Some("secret123".to_string()),
            ..Default::default()
        };

        let mut headers = HeaderMap::new();
        headers.insert("Authorization", "Bearer secret123".parse().unwrap());
        assert!(auth.is_authorized(&headers));

        let mut bad_headers = HeaderMap::new();
        bad_headers.insert("Authorization", "Bearer wrong".parse().unwrap());
        assert!(!auth.is_authorized(&bad_headers));
    }

    #[test]
    fn test_auth_basic() {
        let auth = AuthConfig {
            enabled: true,
            username: Some("admin".to_string()),
            password: Some("secret".to_string()),
            ..Default::default()
        };

        let mut headers = HeaderMap::new();
        // admin:secret in base64 is YWRtaW46c2VjcmV0
        headers.insert("Authorization", "Basic YWRtaW46c2VjcmV0".parse().unwrap());
        assert!(auth.is_authorized(&headers));
    }
}
