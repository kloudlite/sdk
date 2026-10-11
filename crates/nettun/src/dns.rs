//! One A query against cluster DNS, by hand: the daemon needs exactly "name -> first IPv4" for a
//! few dozen names every 10 s, which is not worth a resolver dependency.
// ponytail: A only, no retries, no EDNS; a resolver crate if AAAA or SRV are ever needed.
use std::net::Ipv4Addr;

pub fn a_query(id: u16, name: &str) -> Vec<u8> {
    let mut q = Vec::with_capacity(name.len() + 18);
    q.extend_from_slice(&id.to_be_bytes());
    q.extend_from_slice(&[0x01, 0x00, 0, 1, 0, 0, 0, 0, 0, 0]); // RD, QDCOUNT 1
    for label in name.trim_end_matches('.').split('.') {
        q.push(label.len() as u8);
        q.extend_from_slice(label.as_bytes());
    }
    q.extend_from_slice(&[0, 0, 1, 0, 1]); // root, QTYPE A, QCLASS IN
    q
}

pub fn a_answer(id: u16, r: &[u8]) -> Option<Ipv4Addr> {
    if r.len() < 12 || r[..2] != id.to_be_bytes() || r[2] & 0x80 == 0 || r[3] & 0x0f != 0 {
        return None;
    }
    let qd = u16::from_be_bytes([r[4], r[5]]);
    let an = u16::from_be_bytes([r[6], r[7]]);
    let mut i = 12;
    for _ in 0..qd {
        i = skip_name(r, i)? + 4;
    }
    for _ in 0..an {
        i = skip_name(r, i)?;
        let h = r.get(i..i + 10)?;
        let (ty, len) = (u16::from_be_bytes([h[0], h[1]]), u16::from_be_bytes([h[8], h[9]]) as usize);
        let data = r.get(i + 10..i + 10 + len)?;
        if ty == 1 && len == 4 {
            return Some(Ipv4Addr::new(data[0], data[1], data[2], data[3]));
        }
        i += 10 + len;
    }
    None
}

fn skip_name(r: &[u8], mut i: usize) -> Option<usize> {
    loop {
        let b = *r.get(i)?;
        if b & 0xc0 == 0xc0 {
            return Some(i + 2);
        }
        if b == 0 {
            return Some(i + 1);
        }
        i += 1 + b as usize;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_query_and_answer() {
        let q = a_query(0x1234, "mongodb.env-ab12.svc.cluster.local");
        assert_eq!(&q[..2], &[0x12, 0x34]);
        // reply = query with QR set, ANCOUNT 1, then a compressed-name A answer 10.43.12.7
        let mut r = q.clone();
        r[2] |= 0x80;
        r[7] = 1;
        r.extend_from_slice(&[0xc0, 0x0c, 0, 1, 0, 1, 0, 0, 0, 30, 0, 4, 10, 43, 12, 7]);
        assert_eq!(a_answer(0x1234, &r), Some("10.43.12.7".parse().unwrap()));
        assert_eq!(a_answer(0x9999, &r), None); // id mismatch
        let mut nx = q.clone();
        nx[2] |= 0x80;
        nx[3] |= 3; // RCODE NXDOMAIN
        assert_eq!(a_answer(0x1234, &nx), None);
        assert_eq!(a_answer(0x1234, &r[..20]), None); // truncated
    }
}
