//! Session cookies (HMAC-signed, stateless) and token hashing.

use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use worker::{Request, Result};

type HmacSha256 = Hmac<Sha256>;

pub const SESSION_COOKIE: &str = "shelf_session";
pub const STATE_COOKIE: &str = "shelf_oauth_state";
const SESSION_TTL_SECS: u64 = 30 * 24 * 3600;

pub fn sha256_hex(s: &str) -> String {
    hex::encode(Sha256::digest(s.as_bytes()))
}

pub fn random_hex(bytes: usize) -> Result<String> {
    let mut buf = vec![0u8; bytes];
    getrandom::getrandom(&mut buf).map_err(|e| worker::Error::RustError(e.to_string()))?;
    Ok(hex::encode(buf))
}

fn sign(secret: &str, payload: &str) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).expect("any key length works");
    mac.update(payload.as_bytes());
    mac.finalize().into_bytes().to_vec()
}

/// `<author_id>.<expiry_unix>.<hmac_hex>`
pub fn session_value(secret: &str, author_id: i64, now_ms: u64) -> String {
    let payload = format!("{author_id}.{}", now_ms / 1000 + SESSION_TTL_SECS);
    format!("{payload}.{}", hex::encode(sign(secret, &payload)))
}

pub fn verify_session(secret: &str, value: &str, now_ms: u64) -> Option<i64> {
    let mut parts = value.splitn(3, '.');
    let (id, exp, sig) = (parts.next()?, parts.next()?, parts.next()?);
    let payload = format!("{id}.{exp}");
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).ok()?;
    mac.update(payload.as_bytes());
    mac.verify_slice(&hex::decode(sig).ok()?).ok()?;
    if exp.parse::<u64>().ok()? < now_ms / 1000 {
        return None;
    }
    id.parse().ok()
}

pub fn cookie(req: &Request, name: &str) -> Option<String> {
    let header = req.headers().get("cookie").ok().flatten()?;
    header.split(';').find_map(|part| {
        let (k, v) = part.trim().split_once('=')?;
        (k == name).then(|| v.to_string())
    })
}

pub fn set_cookie(name: &str, value: &str, max_age: u64) -> String {
    format!("{name}={value}; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age={max_age}")
}

pub fn clear_cookie(name: &str) -> String {
    set_cookie(name, "", 0)
}

pub fn bearer_token(req: &Request) -> Option<String> {
    let header = req.headers().get("authorization").ok().flatten()?;
    header.strip_prefix("Bearer ").map(|t| t.trim().to_string())
}
