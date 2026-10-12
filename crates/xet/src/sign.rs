//! HMAC-signed byte URLs. The reconstruction answer and the LFS batch both hand clients URLs they
//! fetch without a token; the signature binds the path and the expiry, so a URL is good for one
//! object, for one hour, and verifies on any node (the key is the cluster's jwt secret).
use hmac::{Hmac, Mac};
use sha2::Sha256;

type H = Hmac<Sha256>;

fn mac(secret: &[u8], path: &str, exp: u64) -> H {
    #[allow(clippy::expect_used)] // HMAC accepts any key length
    let mut m = H::new_from_slice(secret).expect("hmac takes any key");
    m.update(path.as_bytes());
    m.update(b"\n");
    m.update(exp.to_string().as_bytes());
    m
}

pub fn sign(secret: &[u8], path: &str, exp: u64) -> String {
    hex::encode(mac(secret, path, exp).finalize().into_bytes())
}

pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn signed_url(base: &str, secret: &[u8], path: &str, ttl: u64) -> String {
    let exp = now() + ttl;
    format!("{base}{path}?exp={exp}&sig={}", sign(secret, path, exp))
}

pub fn verify(secret: &[u8], path: &str, exp: u64, sig: &str, now: u64) -> bool {
    if now > exp {
        return false;
    }
    let Ok(raw) = hex::decode(sig) else { return false };
    mac(secret, path, exp).verify_slice(&raw).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    const K: &[u8] = b"0123456789abcdef0123456789abcdef";

    #[test]
    fn signature_binds_path_and_expiry() {
        let s = sign(K, "/v1/fetch/alice/abc", 100);
        assert!(verify(K, "/v1/fetch/alice/abc", 100, &s, 50));
        assert!(!verify(K, "/v1/fetch/alice/abc", 100, &s, 101), "expired");
        assert!(!verify(K, "/v1/fetch/bob/abc", 100, &s, 50), "other path");
        assert!(!verify(K, "/v1/fetch/alice/abc", 200, &s, 50), "moved expiry");
        assert!(!verify(K, "/v1/fetch/alice/abc", 100, "zz", 50), "not hex");
    }

    #[test]
    fn signed_url_shape() {
        let u = signed_url("https://cas.x", K, "/v1/fetch/a/h", 3600);
        assert!(u.starts_with("https://cas.x/v1/fetch/a/h?exp="));
        assert!(u.contains("&sig="));
    }
}
