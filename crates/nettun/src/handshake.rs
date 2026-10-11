//! The first frame each way. The keys are not authentication (the bench session is); they make
//! the bytes WireGuard and bind each stream to one peer address, so a stream cannot inject
//! packets for another stream's address.
use std::net::Ipv4Addr;

pub const NET: Ipv4Addr = Ipv4Addr::new(100, 96, 0, 0);
pub const GATEWAY: Ipv4Addr = Ipv4Addr::new(100, 96, 0, 1);
pub const PREFIX: u8 = 16;

pub struct Hello {
    pub key: [u8; 32],
}
impl Hello {
    pub fn encode(&self) -> [u8; 32] {
        self.key
    }
    pub fn decode(b: &[u8]) -> Option<Hello> {
        Some(Hello { key: b.try_into().ok()? })
    }
}

pub struct Welcome {
    pub key: [u8; 32],
    pub addr: Ipv4Addr,
}
impl Welcome {
    pub fn encode(&self) -> [u8; 36] {
        let mut o = [0u8; 36];
        o[..32].copy_from_slice(&self.key);
        o[32..].copy_from_slice(&self.addr.octets());
        o
    }
    pub fn decode(b: &[u8]) -> Option<Welcome> {
        if b.len() != 36 {
            return None;
        }
        Some(Welcome {
            key: b[..32].try_into().ok()?,
            addr: Ipv4Addr::from(<[u8; 4]>::try_from(&b[32..]).ok()?),
        })
    }
}

/// Host part 2..=65534 of 100.96.0.0/16: .0.0 is the network, .0.1 the bench, .255.255 broadcast.
// ponytail: linear scan from a rotating cursor; fine for a few hundred peers, a free-list if it fills.
pub struct Pool {
    used: std::collections::BTreeSet<u16>,
    next: u16,
}
impl Pool {
    pub fn new() -> Pool {
        Pool { used: Default::default(), next: 2 }
    }
    pub fn take(&mut self) -> Option<Ipv4Addr> {
        for _ in 0..65533u32 {
            let h = self.next;
            self.next = if h >= 65534 { 2 } else { h + 1 };
            if self.used.insert(h) {
                return Some(Ipv4Addr::from(u32::from(NET) | h as u32));
            }
        }
        None
    }
    pub fn free(&mut self, a: Ipv4Addr) {
        self.used.remove(&((u32::from(a) & 0xffff) as u16));
    }
}
impl Default for Pool {
    fn default() -> Self {
        Pool::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hello_and_welcome_round_trip() {
        let h = Hello { key: [3; 32] };
        assert_eq!(Hello::decode(&h.encode()).unwrap().key, [3; 32]);
        let w = Welcome { key: [4; 32], addr: "100.96.0.2".parse().unwrap() };
        let d = Welcome::decode(&w.encode()).unwrap();
        assert_eq!((d.key, d.addr), ([4; 32], w.addr));
        assert!(Hello::decode(&[0; 31]).is_none());
        assert!(Welcome::decode(&[0; 35]).is_none());
    }

    #[test]
    fn addresses_are_unique_and_freed() {
        let mut p = Pool::new();
        let a = p.take().unwrap();
        let b = p.take().unwrap();
        assert_ne!(a, b);
        assert_ne!(a, GATEWAY);
        p.free(a);
        let mut seen = std::collections::HashSet::new();
        while let Some(x) = p.take() {
            assert!(seen.insert(x));
        }
        assert_eq!(seen.len() + 1, 65_533); // b still held
        assert!(!seen.contains(&b) && seen.contains(&a) && !seen.contains(&GATEWAY));
        assert!(!seen.contains(&"100.96.255.255".parse().unwrap()));
    }
}
