//! Users + sessions: persistence + password / session-token primitives.
//!
//! Layering note: the *policy* (which role gets which permission) lives in
//! the node/server layer. This module knows only how to store + verify
//! credentials and how to mint / lookup / revoke opaque session tokens.

use argon2::password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use data_encoding::BASE32_NOPAD;
use rand::RngCore;
use rusqlite::params;
use sha2::{Digest, Sha256};

use crate::{Db, StorageError, StorageResult};

/// Role granted to a user. Wire literal is what's stored in `users.role`
/// and what the API emits in `/whoami`. Adding a role = code change here +
/// migration to relax the CHECK constraint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Operator,
    Admin,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::User => "user",
            Role::Operator => "operator",
            Role::Admin => "admin",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "user" => Some(Role::User),
            "operator" => Some(Role::Operator),
            "admin" => Some(Role::Admin),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct UserRecord {
    pub id: i64,
    pub username: String,
    pub role: Role,
    pub created_at: i64,
    pub last_login: Option<i64>,
}

/// Hash a password using argon2id, PHC string format. Salt is generated
/// per call. Result is safe to store verbatim.
pub fn hash_password(password: &str) -> StorageResult<String> {
    let salt = SaltString::generate(&mut OsRng);
    let argon = Argon2::default();
    argon
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| StorageError::Migration(format!("argon2 hash: {e}")))
}

/// Verify a plaintext password against a stored PHC string. Returns
/// `Ok(true)` on match, `Ok(false)` on mismatch, `Err` only on malformed
/// stored hash.
pub fn verify_password(password: &str, stored: &str) -> StorageResult<bool> {
    let parsed = PasswordHash::new(stored)
        .map_err(|e| StorageError::Migration(format!("argon2 parse: {e}")))?;
    Ok(Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok())
}

/// Generate a fresh opaque session token. 32 random bytes Base32-encoded
/// — URL-safe, copy-paste-safe, ~52 chars. Returns `(plaintext_token,
/// token_hash)`. Only the hash is persisted.
pub fn mint_session_token() -> (String, String) {
    let mut raw = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut raw);
    let plaintext = BASE32_NOPAD.encode(&raw).to_lowercase();
    let hash = hash_session_token(&plaintext);
    (plaintext, hash)
}

pub fn hash_session_token(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    BASE32_NOPAD.encode(&digest).to_lowercase()
}

// ---------------------------------------------------------------------------
// users CRUD
// ---------------------------------------------------------------------------

pub fn count_users(db: &Db) -> StorageResult<i64> {
    let conn = db.get()?;
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM users", [], |r| r.get(0))?;
    Ok(n)
}

pub fn create_user(
    db: &Db,
    username: &str,
    password: &str,
    role: Role,
    now_unix: i64,
) -> StorageResult<UserRecord> {
    let hash = hash_password(password)?;
    let conn = db.get()?;
    conn.execute(
        "INSERT INTO users (username, password_hash, role, created_at) VALUES (?1, ?2, ?3, ?4)",
        params![username, hash, role.as_str(), now_unix],
    )?;
    let id = conn.last_insert_rowid();
    Ok(UserRecord {
        id,
        username: username.into(),
        role,
        created_at: now_unix,
        last_login: None,
    })
}

pub fn get_user_by_username(db: &Db, username: &str) -> StorageResult<Option<UserRecord>> {
    let conn = db.get()?;
    let mut stmt = conn.prepare(
        "SELECT id, username, role, created_at, last_login FROM users WHERE username = ?1",
    )?;
    let mut rows = stmt.query(params![username])?;
    if let Some(r) = rows.next()? {
        Ok(Some(row_to_user(r)?))
    } else {
        Ok(None)
    }
}

pub fn get_user_by_id(db: &Db, id: i64) -> StorageResult<Option<UserRecord>> {
    let conn = db.get()?;
    let mut stmt = conn.prepare(
        "SELECT id, username, role, created_at, last_login FROM users WHERE id = ?1",
    )?;
    let mut rows = stmt.query(params![id])?;
    if let Some(r) = rows.next()? {
        Ok(Some(row_to_user(r)?))
    } else {
        Ok(None)
    }
}

pub fn list_users(db: &Db) -> StorageResult<Vec<UserRecord>> {
    let conn = db.get()?;
    let mut stmt = conn.prepare(
        "SELECT id, username, role, created_at, last_login FROM users ORDER BY username",
    )?;
    let rows = stmt.query_map([], row_to_user)?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

pub fn update_user_role(db: &Db, id: i64, role: Role) -> StorageResult<()> {
    let conn = db.get()?;
    conn.execute(
        "UPDATE users SET role = ?1 WHERE id = ?2",
        params![role.as_str(), id],
    )?;
    Ok(())
}

pub fn set_user_password(db: &Db, id: i64, new_password: &str) -> StorageResult<()> {
    let hash = hash_password(new_password)?;
    let conn = db.get()?;
    conn.execute(
        "UPDATE users SET password_hash = ?1 WHERE id = ?2",
        params![hash, id],
    )?;
    Ok(())
}

pub fn delete_user(db: &Db, id: i64) -> StorageResult<()> {
    let conn = db.get()?;
    conn.execute("DELETE FROM users WHERE id = ?1", params![id])?;
    Ok(())
}

/// Fetch the stored password hash for the given username, if any. Kept
/// separate from `get_user_by_username` so the hash never accidentally
/// surfaces in API responses.
pub fn fetch_password_hash(db: &Db, username: &str) -> StorageResult<Option<(i64, String)>> {
    let conn = db.get()?;
    let mut stmt = conn.prepare("SELECT id, password_hash FROM users WHERE username = ?1")?;
    let mut rows = stmt.query(params![username])?;
    if let Some(r) = rows.next()? {
        Ok(Some((r.get(0)?, r.get(1)?)))
    } else {
        Ok(None)
    }
}

pub fn touch_last_login(db: &Db, id: i64, now_unix: i64) -> StorageResult<()> {
    let conn = db.get()?;
    conn.execute(
        "UPDATE users SET last_login = ?1 WHERE id = ?2",
        params![now_unix, id],
    )?;
    Ok(())
}

fn row_to_user(r: &rusqlite::Row<'_>) -> rusqlite::Result<UserRecord> {
    let role_str: String = r.get(2)?;
    let role = Role::parse(&role_str).unwrap_or(Role::User);
    Ok(UserRecord {
        id: r.get(0)?,
        username: r.get(1)?,
        role,
        created_at: r.get(3)?,
        last_login: r.get(4)?,
    })
}

// ---------------------------------------------------------------------------
// sessions
// ---------------------------------------------------------------------------

pub fn create_session(
    db: &Db,
    user_id: i64,
    token_hash: &str,
    created_at: i64,
    expires_at: i64,
) -> StorageResult<()> {
    let conn = db.get()?;
    conn.execute(
        "INSERT INTO sessions (token_hash, user_id, created_at, expires_at) VALUES (?1, ?2, ?3, ?4)",
        params![token_hash, user_id, created_at, expires_at],
    )?;
    Ok(())
}

/// Look up a session by token hash. Returns the owning user if the
/// session exists and is not expired. Expired sessions are best-effort
/// purged inline.
pub fn lookup_session(
    db: &Db,
    token_hash: &str,
    now_unix: i64,
) -> StorageResult<Option<UserRecord>> {
    let conn = db.get()?;
    let row = conn.query_row(
        "SELECT u.id, u.username, u.role, u.created_at, u.last_login, s.expires_at
         FROM sessions s JOIN users u ON u.id = s.user_id
         WHERE s.token_hash = ?1",
        params![token_hash],
        |r| {
            let exp: i64 = r.get(5)?;
            let role_str: String = r.get(2)?;
            Ok((
                UserRecord {
                    id: r.get(0)?,
                    username: r.get(1)?,
                    role: Role::parse(&role_str).unwrap_or(Role::User),
                    created_at: r.get(3)?,
                    last_login: r.get(4)?,
                },
                exp,
            ))
        },
    );
    match row {
        Ok((user, exp)) => {
            if exp <= now_unix {
                let _ = delete_session(db, token_hash);
                Ok(None)
            } else {
                Ok(Some(user))
            }
        }
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

pub fn refresh_session_expiry(
    db: &Db,
    token_hash: &str,
    new_expires_at: i64,
) -> StorageResult<()> {
    let conn = db.get()?;
    conn.execute(
        "UPDATE sessions SET expires_at = ?1 WHERE token_hash = ?2",
        params![new_expires_at, token_hash],
    )?;
    Ok(())
}

pub fn delete_session(db: &Db, token_hash: &str) -> StorageResult<()> {
    let conn = db.get()?;
    conn.execute(
        "DELETE FROM sessions WHERE token_hash = ?1",
        params![token_hash],
    )?;
    Ok(())
}

pub fn delete_sessions_for_user(db: &Db, user_id: i64) -> StorageResult<()> {
    let conn = db.get()?;
    conn.execute(
        "DELETE FROM sessions WHERE user_id = ?1",
        params![user_id],
    )?;
    Ok(())
}

pub fn prune_expired_sessions(db: &Db, now_unix: i64) -> StorageResult<usize> {
    let conn = db.get()?;
    let n = conn.execute(
        "DELETE FROM sessions WHERE expires_at <= ?1",
        params![now_unix],
    )?;
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::open_in_memory;

    #[test]
    fn hash_verify_round_trip() {
        let h = hash_password("hunter2").unwrap();
        assert!(verify_password("hunter2", &h).unwrap());
        assert!(!verify_password("nope", &h).unwrap());
    }

    #[test]
    fn user_crud_basic() {
        let db = open_in_memory().unwrap();
        assert_eq!(count_users(&db).unwrap(), 0);
        let u = create_user(&db, "alice", "pw", Role::Admin, 100).unwrap();
        assert_eq!(u.role, Role::Admin);
        assert_eq!(count_users(&db).unwrap(), 1);
        let fetched = get_user_by_username(&db, "alice").unwrap().unwrap();
        assert_eq!(fetched.username, "alice");
        assert_eq!(fetched.role, Role::Admin);
        let (id, hash) = fetch_password_hash(&db, "alice").unwrap().unwrap();
        assert_eq!(id, u.id);
        assert!(verify_password("pw", &hash).unwrap());
        update_user_role(&db, u.id, Role::Operator).unwrap();
        assert_eq!(get_user_by_id(&db, u.id).unwrap().unwrap().role, Role::Operator);
    }

    #[test]
    fn session_lifecycle() {
        let db = open_in_memory().unwrap();
        let u = create_user(&db, "bob", "pw", Role::User, 100).unwrap();
        let (token, hash) = mint_session_token();
        create_session(&db, u.id, &hash, 100, 200).unwrap();
        // Live lookup
        let me = lookup_session(&db, &hash_session_token(&token), 150).unwrap().unwrap();
        assert_eq!(me.id, u.id);
        // Expired lookup auto-purges
        assert!(lookup_session(&db, &hash_session_token(&token), 300).unwrap().is_none());
    }
}
