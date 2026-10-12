//! git-lfs pointer text. The exact three-line form is what git-lfs, git-xet and huggingface_hub
//! all write, so it is the only form this server writes or recognises.

pub const POINTER_MAX: usize = 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pointer {
    pub sha256: String,
    pub size: u64,
}

pub fn text(sha256: &str, size: u64) -> String {
    format!("version https://git-lfs.github.com/spec/v1\noid sha256:{sha256}\nsize {size}\n")
}

pub fn parse(b: &[u8]) -> Option<Pointer> {
    if b.len() > POINTER_MAX {
        return None;
    }
    let s = std::str::from_utf8(b).ok()?;
    let mut lines = s.strip_suffix('\n')?.split('\n');
    if lines.next()? != "version https://git-lfs.github.com/spec/v1" {
        return None;
    }
    let sha = lines.next()?.strip_prefix("oid sha256:")?;
    let size = lines.next()?.strip_prefix("size ")?.parse().ok()?;
    if lines.next().is_some() || sha.len() != 64 || !sha.bytes().all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f')) {
        return None;
    }
    Some(Pointer { sha256: sha.to_string(), size })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let sha = "a".repeat(64);
        let t = text(&sha, 42);
        assert_eq!(parse(t.as_bytes()), Some(Pointer { sha256: sha, size: 42 }));
    }

    #[test]
    fn refuses_near_misses() {
        let sha = "a".repeat(64);
        assert!(parse(text(&sha, 1).trim_end().as_bytes()).is_none());
        assert!(parse(text(&"A".repeat(64), 1).as_bytes()).is_none());
        assert!(parse(text(&"a".repeat(63), 1).as_bytes()).is_none());
        assert!(parse(format!("{}extra\n", text(&sha, 1)).as_bytes()).is_none());
        assert!(parse(b"hello world\n").is_none());
    }
}
