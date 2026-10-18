//! Lines for the journal. The `<n>` prefix gives each line its priority
//! (sd-daemon(3)): 3 error, 4 warning, 6 information, 7 debug. With a syslog
//! server set (the `logs.syslog` setting), each line also goes there as an
//! RFC 5424 message, sent by a thread of its own, so a slow or absent server
//! never holds up the program: when the thread falls behind, lines are lost
//! to the server but never to the journal.

use std::fmt::Display;
use std::io::Write;
use std::net::{SocketAddr, TcpStream, ToSocketAddrs, UdpSocket};
use std::sync::Mutex;
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::time::{Duration, Instant};

use chrono::{DateTime, SecondsFormat, Utc};

pub fn error(message: impl Display) {
    line(3, message);
}

pub fn warn(message: impl Display) {
    line(4, message);
}

pub fn info(message: impl Display) {
    line(6, message);
}

pub fn debug(message: impl Display) {
    line(7, message);
}

fn line(severity: u8, message: impl Display) {
    let message = message.to_string();
    eprintln!("<{severity}>{message}");
    if let Some(sink) = SINK.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        // Full: the thread is behind, and this line goes to the journal only.
        let _ = sink.lines.try_send((severity, Utc::now(), message));
    }
}

/// A syslog server: `udp://host:port` or `tcp://host:port` (port 514 when
/// left out).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub tcp: bool,
    /// `host:port`, as `ToSocketAddrs` takes it.
    pub address: String,
}

/// The server `value` names, or `None` when it is not one.
pub fn target(value: &str) -> Option<Target> {
    let url = reqwest::Url::parse(value).ok()?;
    let tcp = match url.scheme() {
        "udp" => false,
        "tcp" => true,
        _ => return None,
    };
    let host = url.host_str().filter(|h| !h.is_empty())?;
    let port = url.port().unwrap_or(514);
    let plain = url.username().is_empty()
        && url.password().is_none()
        && matches!(url.path(), "" | "/")
        && url.query().is_none()
        && url.fragment().is_none();
    (plain && port != 0).then(|| Target {
        tcp,
        address: format!("{host}:{port}"),
    })
}

struct Sink {
    /// The setting's text, to tell a change from the same value again.
    setting: String,
    lines: SyncSender<(u8, DateTime<Utc>, String)>,
}

static SINK: Mutex<Option<Sink>> = Mutex::new(None);

/// How many lines may wait for the sending thread.
const QUEUE: usize = 1000;
/// After a server cannot be reached, how long before it is tried again.
const RETRY: Duration = Duration::from_secs(30);

/// From now on, sends each line to the server `setting` names as `app`
/// (such as `scannerpulse-web`), or to the journal only when it is `None` or
/// names no server. The same setting again changes nothing.
pub fn syslog(setting: Option<&str>, app: &str) {
    let mut sink = SINK.lock().unwrap_or_else(|e| e.into_inner());
    if sink.as_ref().map(|s| s.setting.as_str()) == setting {
        return;
    }
    // Dropping the old sender ends its thread once the queue is sent.
    *sink = None;
    let Some((setting, server)) = setting.and_then(|s| Some((s, target(s)?))) else {
        return;
    };
    let (lines, queue) = sync_channel(QUEUE);
    let host = std::env::var("SCANNERPULSE_HOST")
        .ok()
        .filter(|h| !h.is_empty() && h.is_ascii() && !h.contains(' '))
        .unwrap_or_else(|| "-".into());
    let app = app.to_string();
    std::thread::spawn(move || send(server, host, app, queue));
    *sink = Some(Sink {
        setting: setting.to_string(),
        lines,
    });
}

/// One RFC 5424 message: facility daemon (3), no message ID, no
/// structured data.
pub fn message(
    severity: u8,
    at: DateTime<Utc>,
    host: &str,
    app: &str,
    pid: u32,
    text: &str,
) -> String {
    format!(
        "<{}>1 {} {host} {app} {pid} - - {text}",
        3 * 8 + u32::from(severity),
        at.to_rfc3339_opts(SecondsFormat::Millis, true),
    )
}

fn send(server: Target, host: String, app: String, queue: Receiver<(u8, DateTime<Utc>, String)>) {
    let pid = std::process::id();
    let mut failed_at: Option<Instant> = None;
    let mut tcp: Option<TcpStream> = None;
    let mut udp: Option<(UdpSocket, SocketAddr)> = None;
    for (severity, at, text) in queue {
        let text = message(severity, at, &host, &app, pid, &text);
        let waiting = failed_at.is_some_and(|at| at.elapsed() < RETRY);
        let sent = if server.tcp {
            if tcp.is_none() && !waiting {
                tcp = resolve(&server.address).and_then(connect);
            }
            // Octet counting (RFC 6587): the length, a space, the message.
            tcp.as_mut()
                .is_some_and(|stream| write!(stream, "{} {text}", text.len()).is_ok())
        } else {
            if udp.is_none() && !waiting {
                udp = resolve(&server.address).and_then(|to| {
                    let any: SocketAddr = if to.is_ipv6() { "[::]:0" } else { "0.0.0.0:0" }
                        .parse()
                        .expect("an address");
                    UdpSocket::bind(any).ok().map(|socket| (socket, to))
                });
            }
            udp.as_ref()
                .is_some_and(|(socket, to)| socket.send_to(datagram(&text).as_bytes(), to).is_ok())
        };
        if sent {
            failed_at = None;
        } else {
            tcp = None;
            udp = None;
            if !waiting {
                failed_at = Some(Instant::now());
            }
        }
    }
}

/// How long one write to a TCP server may wait: a server that stops reading
/// must not stop the sender for good.
const WRITE_LIMIT: Duration = Duration::from_secs(5);

fn connect(to: SocketAddr) -> Option<TcpStream> {
    let stream = TcpStream::connect_timeout(&to, Duration::from_secs(2)).ok()?;
    stream.set_write_timeout(Some(WRITE_LIMIT)).ok()?;
    Some(stream)
}

/// The longest UDP message: what RFC 5426 says every receiver should take.
/// A longer line is cut there (the journal keeps it whole).
const DATAGRAM: usize = 2048;

fn datagram(text: &str) -> &str {
    let mut end = text.len().min(DATAGRAM);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

fn resolve(address: &str) -> Option<SocketAddr> {
    address.to_socket_addrs().ok()?.next()
}

#[cfg(test)]
mod tests {
    use super::{Target, connect, datagram, message, syslog, target};
    use chrono::{TimeZone, Utc};
    use std::io::{BufRead, BufReader, Read};
    use std::net::{TcpListener, UdpSocket};
    use std::time::Duration;

    #[test]
    fn a_line_is_an_rfc_5424_message_from_the_daemon_facility() {
        let at = Utc.with_ymd_and_hms(2026, 10, 17, 23, 50, 1).unwrap();
        assert_eq!(
            message(4, at, "radio-a", "scannerpulse-web", 812, "a disk is full"),
            "<28>1 2026-10-17T23:50:01.000Z radio-a scannerpulse-web 812 - - a disk is full"
        );
    }

    #[test]
    fn a_server_is_named_by_its_protocol_host_and_port() {
        assert_eq!(
            target("udp://192.0.2.30"),
            Some(Target {
                tcp: false,
                address: "192.0.2.30:514".into()
            })
        );
        assert_eq!(
            target("tcp://[2001:db8::30]:6514"),
            Some(Target {
                tcp: true,
                address: "[2001:db8::30]:6514".into()
            })
        );
        assert_eq!(target("192.0.2.30:514"), None);
    }

    #[test]
    fn a_udp_line_is_cut_to_what_every_receiver_takes() {
        let short = "é".repeat(10);
        assert_eq!(datagram(&short), short);
        let long = format!("a{}", "é".repeat(2000));
        let cut = datagram(&long);
        assert_eq!(cut.len(), 2047, "cut on a character's edge");
        assert!(long.starts_with(cut));
    }

    #[test]
    fn a_tcp_server_that_stops_reading_holds_a_line_for_5_seconds_at_most() {
        let server = TcpListener::bind("127.0.0.1:0").unwrap();
        let stream = connect(server.local_addr().unwrap()).unwrap();
        assert_eq!(
            stream.write_timeout().unwrap(),
            Some(Duration::from_secs(5))
        );
    }

    // One test for both, since the server is the process's own setting.
    #[test]
    fn lines_reach_a_udp_or_tcp_server_and_stop_when_it_is_unset() {
        let udp = UdpSocket::bind("127.0.0.1:0").unwrap();
        udp.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        syslog(
            Some(&format!("udp://{}", udp.local_addr().unwrap())),
            "scannerpulse-test",
        );
        super::warn("over udp");
        let mut got = [0u8; 512];
        let n = udp.recv(&mut got).unwrap();
        let got = String::from_utf8_lossy(&got[..n]);
        assert!(got.starts_with("<28>1 "), "{got}");
        assert!(
            got.ends_with(&format!(
                " scannerpulse-test {} - - over udp",
                std::process::id()
            )),
            "{got}"
        );

        let tcp = TcpListener::bind("127.0.0.1:0").unwrap();
        syslog(
            Some(&format!("tcp://{}", tcp.local_addr().unwrap())),
            "scannerpulse-test",
        );
        super::error("over tcp");
        let (stream, _) = tcp.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut reader = BufReader::new(stream);
        let mut length = Vec::new();
        reader.read_until(b' ', &mut length).unwrap();
        let length: usize = String::from_utf8(length).unwrap().trim().parse().unwrap();
        let mut text = vec![0u8; length];
        reader.read_exact(&mut text).unwrap();
        let text = String::from_utf8(text).unwrap();
        assert!(
            text.starts_with("<27>1 ") && text.ends_with("- - over tcp"),
            "{text}"
        );

        syslog(None, "scannerpulse-test");
        super::info("journal only");
        udp.set_read_timeout(Some(Duration::from_millis(300)))
            .unwrap();
        assert!(udp.recv(&mut [0u8; 64]).is_err(), "nothing more over udp");
    }
}
