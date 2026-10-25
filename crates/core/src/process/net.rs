//! Network traffic per process.
//!
//! The kernel counts the bytes of every TCP connection (`tcp_info`). One
//! `NETLINK_SOCK_DIAG` dump returns all of them with each socket's inode, and
//! `/proc/<pid>/fd` links (`socket:[inode]`) tell which process owns which
//! socket. The difference between two dumps, summed per process, is its
//! speed. UDP (and so QUIC) has no such counters and is not included, and
//! without root only the user's own processes can be matched to sockets.

use std::collections::{HashMap, HashSet};
use std::io;
use std::time::Instant;

/// Bytes sent (acknowledged by the peer) and received on one socket.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SocketBytes {
    pub sent: u64,
    pub received: u64,
}

/// Scans between two reads of every process's sockets. Sockets opened in
/// between count from the next read on.
pub const OWNERS_EVERY: u32 = 3;

const SOCK_DIAG_BY_FAMILY: u16 = 20;
const NLMSG_ERROR: u16 = 2;
const NLMSG_DONE: u16 = 3;
const INET_DIAG_INFO: u16 = 2;
const NLMSG_HDR: usize = 16;
/// `struct inet_diag_msg`; the inode is its last field.
const DIAG_MSG: usize = 72;
/// `bytes_acked` and `bytes_received` in `struct tcp_info`.
const BYTES_ACKED: usize = 120;
const BYTES_RECEIVED: usize = 128;

/// Every TCP socket's byte counters by inode, IPv4 and IPv6.
pub fn tcp_sockets() -> io::Result<HashMap<u32, SocketBytes>> {
    let socket = NetlinkSocket::open()?;
    let mut out = HashMap::new();
    for (seq, family) in [(1, libc::AF_INET), (2, libc::AF_INET6)] {
        socket.send(&dump_request(family as u8, seq))?;
        let mut buf = vec![0u8; 32 * 1024];
        loop {
            let n = socket.recv(&mut buf)?;
            if n == 0 || parse_dump(&buf[..n], &mut out) {
                break;
            }
        }
    }
    Ok(out)
}

/// A `SOCK_DIAG_BY_FAMILY` dump of every TCP socket in every state, asking
/// for `tcp_info`.
fn dump_request(family: u8, seq: u32) -> Vec<u8> {
    let len = NLMSG_HDR + 56;
    let mut msg = Vec::with_capacity(len);
    msg.extend_from_slice(&(len as u32).to_ne_bytes());
    msg.extend_from_slice(&SOCK_DIAG_BY_FAMILY.to_ne_bytes());
    let flags = (libc::NLM_F_REQUEST | libc::NLM_F_DUMP) as u16;
    msg.extend_from_slice(&flags.to_ne_bytes());
    msg.extend_from_slice(&seq.to_ne_bytes());
    msg.extend_from_slice(&0u32.to_ne_bytes());
    // struct inet_diag_req_v2
    msg.push(family);
    msg.push(libc::IPPROTO_TCP as u8);
    msg.push(1 << (INET_DIAG_INFO - 1));
    msg.push(0);
    msg.extend_from_slice(&u32::MAX.to_ne_bytes()); // all states
    msg.resize(len, 0); // struct inet_diag_sockid: match any
    msg
}

/// Add the sockets of one received buffer to `out`. Returns `true` at the
/// end of the dump.
pub fn parse_dump(mut buf: &[u8], out: &mut HashMap<u32, SocketBytes>) -> bool {
    while buf.len() >= NLMSG_HDR {
        let len = u32_at(buf, 0) as usize;
        let kind = u16::from_ne_bytes([buf[4], buf[5]]);
        if len < NLMSG_HDR || len > buf.len() {
            return true;
        }
        if kind == NLMSG_DONE || kind == NLMSG_ERROR {
            return true;
        }
        let body = &buf[NLMSG_HDR..len];
        if body.len() >= DIAG_MSG {
            let inode = u32_at(body, DIAG_MSG - 4);
            if let Some(bytes) = tcp_info(&body[DIAG_MSG..]) {
                if inode != 0 {
                    out.insert(inode, bytes);
                }
            }
        }
        buf = &buf[align(len).min(buf.len())..];
    }
    false
}

/// The counters in the `INET_DIAG_INFO` attribute.
fn tcp_info(mut attrs: &[u8]) -> Option<SocketBytes> {
    while attrs.len() >= 4 {
        let len = usize::from(u16::from_ne_bytes([attrs[0], attrs[1]]));
        let kind = u16::from_ne_bytes([attrs[2], attrs[3]]);
        if len < 4 || len > attrs.len() {
            return None;
        }
        let data = &attrs[4..len];
        if kind == INET_DIAG_INFO && data.len() >= BYTES_RECEIVED + 8 {
            return Some(SocketBytes {
                sent: u64_at(data, BYTES_ACKED),
                received: u64_at(data, BYTES_RECEIVED),
            });
        }
        attrs = &attrs[align(len).min(attrs.len())..];
    }
    None
}

fn align(len: usize) -> usize {
    (len + 3) & !3
}

fn u32_at(buf: &[u8], at: usize) -> u32 {
    let mut b = [0; 4];
    b.copy_from_slice(&buf[at..at + 4]);
    u32::from_ne_bytes(b)
}

fn u64_at(buf: &[u8], at: usize) -> u64 {
    let mut b = [0; 8];
    b.copy_from_slice(&buf[at..at + 8]);
    u64::from_ne_bytes(b)
}

struct NetlinkSocket(libc::c_int);

impl NetlinkSocket {
    fn open() -> io::Result<Self> {
        // SAFETY: plain socket(2); the descriptor is owned by the returned value.
        let fd = unsafe {
            libc::socket(
                libc::AF_NETLINK,
                libc::SOCK_DGRAM | libc::SOCK_CLOEXEC,
                libc::NETLINK_SOCK_DIAG,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(fd))
    }

    fn send(&self, msg: &[u8]) -> io::Result<()> {
        // SAFETY: an all-zero sockaddr_nl addresses the kernel.
        let mut addr: libc::sockaddr_nl = unsafe { std::mem::zeroed() };
        addr.nl_family = libc::AF_NETLINK as libc::sa_family_t;
        // SAFETY: msg and addr outlive the call; lengths match.
        let n = unsafe {
            libc::sendto(
                self.0,
                msg.as_ptr().cast(),
                msg.len(),
                0,
                std::ptr::addr_of!(addr).cast(),
                std::mem::size_of::<libc::sockaddr_nl>() as libc::socklen_t,
            )
        };
        if n < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    fn recv(&self, buf: &mut [u8]) -> io::Result<usize> {
        // SAFETY: buf is valid for writes of its length.
        let n = unsafe { libc::recv(self.0, buf.as_mut_ptr().cast(), buf.len(), 0) };
        usize::try_from(n).map_err(|_| io::Error::last_os_error())
    }
}

impl Drop for NetlinkSocket {
    fn drop(&mut self) {
        // SAFETY: the descriptor is ours and closed once.
        unsafe { libc::close(self.0) };
    }
}

/// Which process owns which socket, and whose sockets could be read.
#[derive(Debug, Default)]
pub struct Owners {
    /// Socket inode to pid.
    pub sockets: HashMap<u32, u32>,
    /// Pids whose descriptors could be read: other users' processes show no
    /// speed rather than a wrong zero.
    pub readable: HashSet<u32>,
}

/// [`Owners`] from `ls -l /proc/[0-9]*/fd`, as filtered by the Flatpak host
/// script: `D /proc/123/fd:` starts a readable process and `F 4567]` is one
/// of its sockets.
pub fn parse_fd_listing<'a>(lines: impl IntoIterator<Item = &'a str>) -> Owners {
    let mut owners = Owners::default();
    let mut pid = None;
    for line in lines {
        if let Some(dir) = line.strip_prefix("D ") {
            pid = dir
                .strip_prefix("/proc/")
                .and_then(|d| d.split('/').next())
                .and_then(|p| p.parse().ok());
            owners.readable.extend(pid);
        } else if let (Some(inode), Some(pid)) = (line.strip_prefix("F "), pid) {
            if let Ok(inode) = inode.trim_end_matches(']').parse() {
                owners.sockets.entry(inode).or_insert(pid);
            }
        }
    }
    owners
}

/// Turns socket counters into speeds per process.
#[derive(Default)]
pub struct NetTracker {
    prev: HashMap<u32, SocketBytes>,
    prev_time: Option<Instant>,
    pub owners: Owners,
    pub owners_countdown: u32,
}

impl NetTracker {
    /// Forget the last counters, so the next speeds start fresh.
    pub fn pause(&mut self) {
        self.prev.clear();
        self.prev_time = None;
        self.owners_countdown = 0;
    }

    /// Bytes per second received and sent, by pid, since the last call.
    pub fn rates(&mut self, now: HashMap<u32, SocketBytes>) -> HashMap<u32, (u64, u64)> {
        let time = Instant::now();
        let secs = self
            .prev_time
            .map(|t| time.duration_since(t).as_secs_f64())
            .filter(|s| *s > 0.0);
        let mut rates: HashMap<u32, (u64, u64)> = HashMap::new();
        if let Some(secs) = secs {
            for (inode, bytes) in &now {
                let (Some(before), Some(pid)) =
                    (self.prev.get(inode), self.owners.sockets.get(inode))
                else {
                    continue;
                };
                let rate = rates.entry(*pid).or_default();
                rate.0 += (bytes.received.saturating_sub(before.received) as f64 / secs) as u64;
                rate.1 += (bytes.sent.saturating_sub(before.sent) as f64 / secs) as u64;
            }
        }
        self.prev = now;
        self.prev_time = Some(time);
        rates
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn diag_message(inode: u32, sent: u64, received: u64) -> Vec<u8> {
        let mut info = vec![0u8; 232];
        info[BYTES_ACKED..BYTES_ACKED + 8].copy_from_slice(&sent.to_ne_bytes());
        info[BYTES_RECEIVED..BYTES_RECEIVED + 8].copy_from_slice(&received.to_ne_bytes());
        let mut body = vec![0u8; DIAG_MSG];
        body[DIAG_MSG - 4..].copy_from_slice(&inode.to_ne_bytes());
        // A different attribute first, to check the walk.
        body.extend_from_slice(&8u16.to_ne_bytes());
        body.extend_from_slice(&1u16.to_ne_bytes());
        body.extend_from_slice(&[0; 4]);
        body.extend_from_slice(&((4 + info.len()) as u16).to_ne_bytes());
        body.extend_from_slice(&INET_DIAG_INFO.to_ne_bytes());
        body.extend_from_slice(&info);
        let mut msg = Vec::new();
        msg.extend_from_slice(&((NLMSG_HDR + body.len()) as u32).to_ne_bytes());
        msg.extend_from_slice(&SOCK_DIAG_BY_FAMILY.to_ne_bytes());
        msg.extend_from_slice(&[0; 10]);
        msg.extend_from_slice(&body);
        msg
    }

    #[test]
    fn dump_messages_give_socket_counters() {
        let mut buf = diag_message(42, 1000, 5000);
        buf.extend(diag_message(43, 1, 2));
        let mut out = HashMap::new();
        assert!(!parse_dump(&buf, &mut out));
        assert_eq!(
            out[&42],
            SocketBytes {
                sent: 1000,
                received: 5000
            }
        );
        assert_eq!(out[&43].received, 2);

        let mut done = vec![0u8; NLMSG_HDR];
        done[..4].copy_from_slice(&(NLMSG_HDR as u32).to_ne_bytes());
        done[4..6].copy_from_slice(&NLMSG_DONE.to_ne_bytes());
        assert!(parse_dump(&done, &mut out));
    }

    #[test]
    fn fd_listing_maps_sockets_to_processes() {
        let owners = parse_fd_listing([
            "D /proc/12/fd:",
            "F 100]",
            "F 101]",
            "D /proc/34/fd:",
            "F 200]",
        ]);
        assert_eq!(owners.sockets[&100], 12);
        assert_eq!(owners.sockets[&101], 12);
        assert_eq!(owners.sockets[&200], 34);
        assert!(owners.readable.contains(&34));
    }

    #[test]
    fn rates_follow_the_counters() {
        let mut tracker = NetTracker::default();
        tracker.owners.sockets.insert(7, 1);
        let bytes = |sent, received| SocketBytes { sent, received };
        assert!(tracker.rates(HashMap::from([(7, bytes(0, 0))])).is_empty());
        std::thread::sleep(std::time::Duration::from_millis(50));
        let rates = tracker.rates(HashMap::from([(7, bytes(1000, 2000))]));
        let (rx, tx) = rates[&1];
        assert!(rx > tx && tx > 0);
    }

    #[test]
    fn the_kernel_counts_a_local_connection() {
        use std::io::{Read, Write};
        use std::os::fd::AsRawFd;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let mut client = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (mut server, _) = listener.accept().unwrap();
        client.write_all(&[7; 5000]).unwrap();
        let mut buf = [0; 5000];
        server.read_exact(&mut buf).unwrap();
        let link = std::fs::read_link(format!("/proc/self/fd/{}", client.as_raw_fd())).unwrap();
        let inode: u32 = link
            .to_str()
            .and_then(|t| t.strip_prefix("socket:["))
            .and_then(|t| t.strip_suffix(']'))
            .and_then(|t| t.parse().ok())
            .unwrap();
        let sockets = match tcp_sockets() {
            Ok(s) => s,
            // Some build sandboxes have no sock_diag.
            Err(e) => return eprintln!("no sock_diag: {e}"),
        };
        // bytes_acked counts the SYN as one byte; speeds use differences.
        assert!((5000..=5001).contains(&sockets[&inode].sent));
    }
}
