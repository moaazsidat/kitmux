//! kitty's remote control protocol, spoken directly over its socket.
//!
//! A request is `ESC P @kitty-cmd <json> ESC \`, and so is the reply. Talking
//! to the socket rather than shelling out to `kitty @` means kit works from a
//! systemd timer, where the kitty binary is often not on PATH.
//!
//! Only unencrypted sockets are supported: `allow_remote_control yes` (or
//! `socket-only`) with `listen_on unix:...`. A `remote_control_password`
//! requires the encrypted form of the protocol, which kit does not implement.

use std::fmt;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};

const PREFIX: &[u8] = b"\x1bP@kitty-cmd";
const SUFFIX: &[u8] = b"\x1b\\";

/// The protocol version kit speaks. kitty accepts requests from older clients.
const VERSION: [u32; 3] = [0, 43, 0];

/// A kitty does not normally take long to answer, but `launch` waits for a
/// window to be created, so allow a generous bound before giving up.
const TIMEOUT: Duration = Duration::from_secs(10);

/// Where a kitty instance listens.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Addr {
    /// A socket file, `unix:/path`.
    Path(PathBuf),
    /// A Linux abstract socket, `unix:@name`.
    Abstract(String),
}

impl Addr {
    /// Parses a `listen_on` / `KITTY_LISTEN_ON` value such as `unix:/tmp/kitty`.
    pub fn parse(s: &str) -> Result<Addr> {
        let rest = s
            .strip_prefix("unix:")
            .ok_or_else(|| anyhow!("unsupported kitty address {s:?}: only unix: sockets are supported"))?;
        if let Some(name) = rest.strip_prefix('@') {
            Ok(Addr::Abstract(name.to_string()))
        } else {
            Ok(Addr::Path(PathBuf::from(rest)))
        }
    }

    fn connect(&self) -> std::io::Result<UnixStream> {
        match self {
            Addr::Path(p) => UnixStream::connect(p),
            Addr::Abstract(name) => connect_abstract(name),
        }
    }
}

impl fmt::Display for Addr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Addr::Path(p) => write!(f, "unix:{}", p.display()),
            Addr::Abstract(n) => write!(f, "unix:@{n}"),
        }
    }
}

#[cfg(target_os = "linux")]
fn connect_abstract(name: &str) -> std::io::Result<UnixStream> {
    use std::os::linux::net::SocketAddrExt;
    let addr = std::os::unix::net::SocketAddr::from_abstract_name(name.as_bytes())?;
    UnixStream::connect_addr(&addr)
}

#[cfg(not(target_os = "linux"))]
fn connect_abstract(_name: &str) -> std::io::Result<UnixStream> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "abstract sockets exist only on Linux",
    ))
}

/// A connection target for remote control commands. Each command opens its own
/// connection, as `kitty @` does.
#[derive(Debug, Clone)]
pub struct Client {
    pub addr: Addr,
}

impl Client {
    pub fn new(addr: Addr) -> Client {
        Client { addr }
    }

    /// Sends one command and returns its `data`, or `Value::Null` when kitty
    /// sent none.
    pub fn call(&self, cmd: &str, payload: Value) -> Result<Value> {
        let mut req = json!({ "cmd": cmd, "version": VERSION, "no_response": false });
        if !payload.is_null() {
            req["payload"] = payload;
        }
        let mut stream = self
            .addr
            .connect()
            .with_context(|| format!("connecting to kitty at {}", self.addr))?;
        stream.set_read_timeout(Some(TIMEOUT))?;
        stream.set_write_timeout(Some(TIMEOUT))?;

        let mut msg = PREFIX.to_vec();
        msg.extend(serde_json::to_vec(&req)?);
        msg.extend(SUFFIX);
        stream.write_all(&msg)?;

        let mut buf = Vec::new();
        let mut chunk = [0u8; 64 * 1024];
        while !buf.ends_with(SUFFIX) {
            let n = stream
                .read(&mut chunk)
                .with_context(|| format!("reading kitty's reply to {cmd}"))?;
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
        }
        decode_reply(cmd, &buf)
    }

    /// `kitty @ ls`: the full tree of OS windows, tabs, and windows.
    pub fn ls(&self) -> Result<Value> {
        match self.call("ls", Value::Null)? {
            Value::String(s) => serde_json::from_str(&s).context("parsing kitty's ls output"),
            other => Ok(other),
        }
    }

    /// `kitty @ launch`, returning the new window's id.
    pub fn launch(&self, payload: Value) -> Result<u64> {
        let data = self.call("launch", payload)?;
        let id = match &data {
            Value::String(s) => s.trim().parse().ok(),
            Value::Number(n) => n.as_u64(),
            _ => None,
        };
        id.ok_or_else(|| anyhow!("kitty's launch returned no window id: {data}"))
    }
}

fn decode_reply(cmd: &str, buf: &[u8]) -> Result<Value> {
    let body = buf
        .strip_prefix(PREFIX)
        .and_then(|b| b.strip_suffix(SUFFIX))
        .ok_or_else(|| anyhow!("malformed reply from kitty to {cmd}"))?;
    let reply: Value = serde_json::from_slice(body).context("parsing kitty's reply")?;
    if reply.get("ok").and_then(Value::as_bool) != Some(true) {
        let err = reply.get("error").and_then(Value::as_str).unwrap_or("unknown error");
        bail!("kitty refused {cmd}: {err}");
    }
    Ok(reply.get("data").cloned().unwrap_or(Value::Null))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_addresses() {
        assert_eq!(Addr::parse("unix:/tmp/kitty").unwrap(), Addr::Path("/tmp/kitty".into()));
        assert_eq!(Addr::parse("unix:@mykitty").unwrap(), Addr::Abstract("mykitty".into()));
        assert!(Addr::parse("tcp:localhost:12345").is_err());
    }

    #[test]
    fn decodes_replies() {
        let ok = b"\x1bP@kitty-cmd{\"ok\": true, \"data\": \"7\"}\x1b\\";
        assert_eq!(decode_reply("launch", ok).unwrap(), Value::String("7".into()));

        let empty = b"\x1bP@kitty-cmd{\"ok\": true}\x1b\\";
        assert_eq!(decode_reply("goto-layout", empty).unwrap(), Value::Null);

        let err = b"\x1bP@kitty-cmd{\"ok\": false, \"error\": \"No matching windows\"}\x1b\\";
        let e = decode_reply("focus-window", err).unwrap_err().to_string();
        assert!(e.contains("No matching windows"), "{e}");

        assert!(decode_reply("ls", b"garbage").is_err());
    }
}
