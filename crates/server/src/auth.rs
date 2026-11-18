//! Anmeldung (ein Admin-Konto, Ersteinrichtung), Sitzungen und API-Token.

use argon2::{
    Argon2,
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
};
use axum::{
    Json,
    extract::{FromRequestParts, Path, State},
    http::{HeaderMap, HeaderValue, header, request::Parts},
    response::{IntoResponse, Response},
};
use nrz_gp_common::{ApiToken, AuthState, ChangePassword, CreateToken, CreatedToken, Credentials, Ok as OkResp};
use sha2::{Digest, Sha256};

use crate::{
    error::{AppError, AppResult},
    state::AppState,
    store::{Session, StoredToken, User, new_id, now},
};

pub const COOKIE: &str = "nrzgp_session";
const SESSION_DAYS: i64 = 30;

pub fn sha256_hex(s: &str) -> String {
    hex::encode(Sha256::digest(s.as_bytes()))
}

fn hash_password(pw: &str) -> AppResult<String> {
    let mut bytes = [0u8; 16];
    rand::RngCore::fill_bytes(&mut rand::rng(), &mut bytes);
    let salt = SaltString::encode_b64(&bytes).map_err(|e| AppError::internal(e.to_string()))?;
    Argon2::default()
        .hash_password(pw.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| AppError::internal(e.to_string()))
}

fn verify_password(pw: &str, hash: &str) -> bool {
    PasswordHash::new(hash).is_ok_and(|h| Argon2::default().verify_password(pw.as_bytes(), &h).is_ok())
}

/// Angemeldeter Benutzer (Cookie oder Bearer-Token). Als Extraktor in jeder geschützten Route.
#[derive(Debug, Clone)]
pub struct Authed {
    pub username: String,
}

fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers.get_all(header::COOKIE).iter().filter_map(|v| v.to_str().ok()).find_map(|c| {
        c.split(';').map(str::trim).find_map(|kv| kv.strip_prefix(name)?.strip_prefix('=').map(str::to_string))
    })
}

/// Prüft Cookie bzw. `Authorization: Bearer`. Auch `?token=` (für WebSocket/Bilder ohne Header).
pub async fn authenticate(state: &AppState, headers: &HeaderMap, query: Option<&str>) -> Option<Authed> {
    if let Some(sess) = cookie_value(headers, COOKIE) {
        let h = sha256_hex(&sess);
        let t = now();
        let user = state.store.read(|s| s.sessions.get(&h).filter(|x| x.expires > t).map(|x| x.username.clone())).await;
        if let Some(username) = user {
            return Some(Authed { username });
        }
    }
    let bearer = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer ").map(str::trim).map(str::to_string))
        .or_else(|| {
            query?.split('&').find_map(|kv| kv.strip_prefix("token=").map(str::to_string))
        })?;
    let h = sha256_hex(&bearer);
    let found = state
        .store
        .update(|s| {
            let user = s.users.first().map(|u| u.username.clone());
            s.tokens.iter_mut().find(|t| t.hash == h).map(|t| {
                t.last_used = Some(now());
                user.unwrap_or_else(|| "api".into())
            })
        })
        .await
        .ok()
        .flatten()?;
    Some(Authed { username: found })
}

impl FromRequestParts<AppState> for Authed {
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Self::Rejection> {
        authenticate(state, &parts.headers, parts.uri.query()).await.ok_or(AppError::Unauthorized)
    }
}

fn session_cookie(value: &str, max_age: i64) -> HeaderValue {
    HeaderValue::from_str(&format!("{COOKIE}={value}; Path=/; HttpOnly; SameSite=Lax; Max-Age={max_age}"))
        .unwrap_or_else(|_| HeaderValue::from_static(""))
}

async fn new_session(state: &AppState, username: &str) -> AppResult<HeaderValue> {
    let token = new_id(32);
    let h = sha256_hex(&token);
    let expires = now() + SESSION_DAYS * 86400;
    let t = now();
    state
        .store
        .update(|s| {
            s.sessions.retain(|_, v| v.expires > t);
            s.sessions.insert(h, Session { username: username.to_string(), expires });
        })
        .await?;
    Ok(session_cookie(&token, SESSION_DAYS * 86400))
}

pub async fn me(State(state): State<AppState>, headers: HeaderMap) -> Json<AuthState> {
    let setup_required = state.store.read(|s| s.users.is_empty()).await;
    let user = authenticate(&state, &headers, None).await.map(|a| a.username);
    Json(AuthState { setup_required, user })
}

pub async fn setup(State(state): State<AppState>, Json(c): Json<Credentials>) -> AppResult<Response> {
    let username = c.username.trim().to_string();
    if username.is_empty() {
        return Err(AppError::bad("Benutzername fehlt"));
    }
    if c.password.len() < 8 {
        return Err(AppError::bad("Passwort muss mindestens 8 Zeichen haben"));
    }
    let hash = hash_password(&c.password)?;
    let created = state
        .store
        .update(|s| {
            if !s.users.is_empty() {
                return false;
            }
            s.users.push(User { username: username.clone(), password_hash: hash });
            true
        })
        .await?;
    if !created {
        return Err(AppError::Forbidden("Panel ist bereits eingerichtet".into()));
    }
    let cookie = new_session(&state, &username).await?;
    Ok(([(header::SET_COOKIE, cookie)], Json(AuthState { setup_required: false, user: Some(username) })).into_response())
}

pub async fn login(State(state): State<AppState>, Json(c): Json<Credentials>) -> AppResult<Response> {
    let user = state.store.read(|s| s.users.iter().find(|u| u.username == c.username.trim()).cloned()).await;
    let Some(user) = user.filter(|u| verify_password(&c.password, &u.password_hash)) else {
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
        return Err(AppError::Forbidden("Benutzername oder Passwort falsch".into()));
    };
    let cookie = new_session(&state, &user.username).await?;
    Ok(([(header::SET_COOKIE, cookie)], Json(AuthState { setup_required: false, user: Some(user.username) })).into_response())
}

pub async fn logout(State(state): State<AppState>, headers: HeaderMap) -> AppResult<Response> {
    if let Some(sess) = cookie_value(&headers, COOKIE) {
        let h = sha256_hex(&sess);
        state.store.update(|s| s.sessions.remove(&h)).await?;
    }
    Ok(([(header::SET_COOKIE, session_cookie("", 0))], Json(OkResp { ok: true })).into_response())
}

pub async fn change_password(State(state): State<AppState>, a: Authed, Json(c): Json<ChangePassword>) -> AppResult<Json<OkResp>> {
    if c.new.len() < 8 {
        return Err(AppError::bad("Neues Passwort muss mindestens 8 Zeichen haben"));
    }
    let user = state.store.read(|s| s.users.iter().find(|u| u.username == a.username).cloned()).await;
    let Some(user) = user.filter(|u| verify_password(&c.current, &u.password_hash)) else {
        return Err(AppError::Forbidden("Aktuelles Passwort falsch".into()));
    };
    let hash = hash_password(&c.new)?;
    state
        .store
        .update(|s| {
            if let Some(u) = s.users.iter_mut().find(|u| u.username == user.username) {
                u.password_hash = hash;
            }
            s.sessions.retain(|_, v| v.username != user.username);
        })
        .await?;
    Ok(Json(OkResp { ok: true }))
}

fn to_api(t: &StoredToken) -> ApiToken {
    ApiToken { id: t.id.clone(), name: t.name.clone(), prefix: t.prefix.clone(), created_at: t.created_at, last_used: t.last_used }
}

pub async fn list_tokens(State(state): State<AppState>, _a: Authed) -> Json<Vec<ApiToken>> {
    Json(state.store.read(|s| s.tokens.iter().map(to_api).collect()).await)
}

pub async fn create_token(State(state): State<AppState>, _a: Authed, Json(c): Json<CreateToken>) -> AppResult<Json<CreatedToken>> {
    let name = if c.name.trim().is_empty() { "Token".to_string() } else { c.name.trim().to_string() };
    let secret = format!("nrzgp_{}", new_id(24));
    let t = StoredToken {
        id: new_id(8),
        name,
        prefix: format!("{}…", &secret[..12]),
        hash: sha256_hex(&secret),
        created_at: now(),
        last_used: None,
    };
    let api = to_api(&t);
    state.store.update(|s| s.tokens.push(t)).await?;
    Ok(Json(CreatedToken { token: api, secret }))
}

pub async fn delete_token(State(state): State<AppState>, _a: Authed, Path(tid): Path<String>) -> AppResult<Json<OkResp>> {
    state.store.update(|s| s.tokens.retain(|t| t.id != tid)).await?;
    Ok(Json(OkResp { ok: true }))
}
