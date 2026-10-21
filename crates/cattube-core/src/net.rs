//! Small network helpers for finding servers on the home network when they
//! don't answer discovery.

use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpStream};
use std::time::Duration;

/// GET `path` from `addr` and return the body (at most `max` bytes): small
/// JSON answers from servers on the home network (discovery fallbacks).
pub fn get_small(
    addr: SocketAddrV4,
    path: &str,
    headers: &[(&str, &str)],
    timeout: Duration,
    max: usize,
) -> io::Result<String> {
    let mut stream = TcpStream::connect_timeout(&SocketAddr::V4(addr), timeout)?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    let clean = |s: &str| -> String { s.chars().filter(|c| *c != '\r' && *c != '\n').collect() };
    let mut head = format!("GET {} HTTP/1.0\r\nHost: {addr}\r\n", clean(path));
    for (k, v) in headers {
        head.push_str(&format!("{}: {}\r\n", clean(k), clean(v)));
    }
    head.push_str("Connection: close\r\n\r\n");
    stream.write_all(head.as_bytes())?;
    let mut buf = Vec::new();
    stream.take(max as u64).read_to_end(&mut buf)?;
    let text = String::from_utf8_lossy(&buf).into_owned();
    let status_ok = text
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .is_some_and(|c| c == "200");
    if !status_ok {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "not 200"));
    }
    Ok(text
        .split_once("\r\n\r\n")
        .map_or(String::new(), |(_, b)| b.to_string()))
}

/// This machine's IPv4 address on the network the default route uses.
pub fn local_ipv4() -> Option<Ipv4Addr> {
    let s = std::net::UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    // No packet is sent: connect only picks the route.
    s.connect((Ipv4Addr::new(10, 255, 255, 255), 9)).ok()?;
    match s.local_addr().ok()? {
        SocketAddr::V4(a) if !a.ip().is_loopback() && !a.ip().is_unspecified() => Some(*a.ip()),
        _ => None,
    }
}

/// The other hosts of this machine's /24 network that accept a connection
/// on `port`, for servers that do not answer discovery (a server in a
/// container often doesn't). A few parallel tries per host, `timeout` each.
pub fn scan_lan(port: u16, timeout: Duration) -> Vec<Ipv4Addr> {
    let Some(me) = local_ipv4() else {
        return Vec::new();
    };
    let [a, b, c, own] = me.octets();
    let hosts: Vec<Ipv4Addr> = (1..=254u8)
        .filter(|&d| d != own)
        .map(|d| Ipv4Addr::new(a, b, c, d))
        .collect();
    let found = std::sync::Mutex::new(Vec::new());
    std::thread::scope(|s| {
        for chunk in hosts.chunks(4) {
            let found = &found;
            s.spawn(move || {
                for ip in chunk {
                    let addr = SocketAddr::V4(SocketAddrV4::new(*ip, port));
                    if TcpStream::connect_timeout(&addr, timeout).is_ok() {
                        found.lock().unwrap_or_else(|e| e.into_inner()).push(*ip);
                    }
                }
            });
        }
    });
    let mut v = found.into_inner().unwrap_or_default();
    v.sort();
    v
}
