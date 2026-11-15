//! Just enough HTTP/1.1 for LocalSend: one request per connection, bodies
//! by length or chunked, and the connection closed after the response.

use std::collections::BTreeMap;
use std::io::{self, BufRead, Read, Write};

/// The most a request or response head may take.
const MAX_HEAD: usize = 64 * 1024;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Head {
    /// `POST /path?query HTTP/1.1`, or `HTTP/1.1 200 OK`.
    pub start: String,
    pub headers: Vec<(String, String)>,
}

impl Head {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    pub fn content_length(&self) -> Option<u64> {
        self.header("content-length")?.trim().parse().ok()
    }

    pub fn chunked(&self) -> bool {
        self.header("transfer-encoding")
            .is_some_and(|v| v.to_ascii_lowercase().contains("chunked"))
    }
}

fn bad(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.to_owned())
}

/// Read a head up to its blank line. `None` when the peer closed first.
pub fn read_head(r: &mut impl BufRead) -> io::Result<Option<Head>> {
    let mut lines = Vec::new();
    let mut total = 0;
    loop {
        let mut line = Vec::new();
        let n = r
            .take((MAX_HEAD - total + 1) as u64)
            .read_until(b'\n', &mut line)?;
        if n == 0 {
            if lines.is_empty() {
                return Ok(None);
            }
            return Err(bad("connection closed in the head"));
        }
        total += n;
        if total > MAX_HEAD {
            return Err(bad("head too large"));
        }
        let text = String::from_utf8_lossy(&line);
        let text = text.trim_end_matches(['\r', '\n']);
        if text.is_empty() {
            if lines.is_empty() {
                // Stray blank lines before a request are allowed.
                continue;
            }
            break;
        }
        lines.push(text.to_owned());
    }
    let mut it = lines.into_iter();
    let start = it.next().unwrap_or_default();
    let headers = it
        .filter_map(|l| {
            let (k, v) = l.split_once(':')?;
            Some((k.trim().to_owned(), v.trim().to_owned()))
        })
        .collect();
    Ok(Some(Head { start, headers }))
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub query: BTreeMap<String, String>,
    pub head: Head,
}

impl Request {
    pub fn parse(head: Head) -> io::Result<Self> {
        let mut parts = head.start.split_whitespace();
        let method = parts.next().ok_or_else(|| bad("no method"))?.to_owned();
        let target = parts.next().ok_or_else(|| bad("no target"))?;
        let (path, query) = target.split_once('?').unwrap_or((target, ""));
        Ok(Self {
            method,
            path: percent_decode(path),
            query: parse_query(query),
            head,
        })
    }

    pub fn param(&self, key: &str) -> Option<&str> {
        self.query.get(key).map(String::as_str)
    }
}

pub fn parse_query(q: &str) -> BTreeMap<String, String> {
    q.split('&')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            (
                percent_decode(&k.replace('+', " ")),
                percent_decode(&v.replace('+', " ")),
            )
        })
        .collect()
}

pub fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
            if let Ok(b) = u8::from_str_radix(hex, 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// For query values: everything but unreserved characters escaped.
pub fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// A chunked body, read as the plain bytes.
pub struct Chunked<R> {
    inner: R,
    left: u64,
    done: bool,
}

impl<R: BufRead> Chunked<R> {
    pub fn new(inner: R) -> Self {
        Self {
            inner,
            left: 0,
            done: false,
        }
    }

    fn line(&mut self) -> io::Result<String> {
        let mut line = Vec::new();
        self.inner
            .by_ref()
            .take(4096)
            .read_until(b'\n', &mut line)?;
        if line.is_empty() {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "chunk"));
        }
        Ok(String::from_utf8_lossy(&line).trim().to_owned())
    }
}

impl<R: BufRead> Read for Chunked<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.done || buf.is_empty() {
            return Ok(0);
        }
        if self.left == 0 {
            let line = self.line()?;
            let size = line.split(';').next().unwrap_or("").trim();
            self.left = u64::from_str_radix(size, 16).map_err(|_| bad("chunk size"))?;
            if self.left == 0 {
                // Trailers, up to the blank line.
                while !self.line()?.is_empty() {}
                self.done = true;
                return Ok(0);
            }
        }
        let want = buf.len().min(self.left as usize);
        let n = self.inner.read(&mut buf[..want])?;
        if n == 0 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "chunk"));
        }
        self.left -= n as u64;
        if self.left == 0 {
            self.line()?;
        }
        Ok(n)
    }
}

/// The body that follows `head`: by its length, chunked, or (`to_end`,
/// for responses) whatever comes until the connection closes.
pub fn body<'a, R: BufRead + 'a>(r: R, head: &Head, to_end: bool) -> Box<dyn Read + 'a> {
    if head.chunked() {
        Box::new(Chunked::new(r))
    } else if let Some(n) = head.content_length() {
        Box::new(r.take(n))
    } else if to_end {
        Box::new(r)
    } else {
        Box::new(io::empty())
    }
}

/// Read a whole body, refusing one larger than `max`.
pub fn read_all(mut body: impl Read, max: usize) -> io::Result<Vec<u8>> {
    let mut out = Vec::new();
    body.by_ref().take(max as u64 + 1).read_to_end(&mut out)?;
    if out.len() > max {
        return Err(bad("body too large"));
    }
    Ok(out)
}

pub fn reason(status: u16) -> &'static str {
    match status {
        100 => "Continue",
        200 => "OK",
        204 => "No Content",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        409 => "Conflict",
        422 => "Unprocessable Entity",
        429 => "Too Many Requests",
        _ => "Internal Server Error",
    }
}

pub fn write_response(
    w: &mut impl Write,
    status: u16,
    content_type: Option<&str>,
    body: &[u8],
) -> io::Result<()> {
    let mut head = format!("HTTP/1.1 {status} {}\r\n", reason(status));
    if let Some(t) = content_type {
        head.push_str(&format!("Content-Type: {t}\r\n"));
    }
    head.push_str(&format!(
        "Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    ));
    w.write_all(head.as_bytes())?;
    w.write_all(body)?;
    w.flush()
}

pub fn status_of(head: &Head) -> io::Result<u16> {
    head.start
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| bad("status line"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufReader;

    #[test]
    fn reads_a_request_and_its_body() {
        let raw = b"POST /api/localsend/v2/upload?sessionId=a%20b&fileId=1&token=x+y HTTP/1.1\r\n\
            Host: 1.2.3.4\r\nContent-Length: 5\r\n\r\nhelloEXTRA";
        let mut r = BufReader::new(&raw[..]);
        let head = read_head(&mut r).unwrap().unwrap();
        let req = Request::parse(head).unwrap();
        assert_eq!(req.method, "POST");
        assert_eq!(req.path, "/api/localsend/v2/upload");
        assert_eq!(req.param("sessionId"), Some("a b"));
        assert_eq!(req.param("token"), Some("x y"));
        let body = read_all(body(&mut r, &req.head, false), 100).unwrap();
        assert_eq!(body, b"hello");
    }

    #[test]
    fn reads_chunked_bodies_and_responses() {
        let raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n\
            4\r\nWiki\r\n5;x=y\r\npedia\r\n0\r\nT: 1\r\n\r\n";
        let mut r = BufReader::new(&raw[..]);
        let head = read_head(&mut r).unwrap().unwrap();
        assert_eq!(status_of(&head).unwrap(), 200);
        let body = read_all(body(&mut r, &head, true), 100).unwrap();
        assert_eq!(body, b"Wikipedia");
        assert!(read_all(&b"toolong"[..], 3).is_err());
        assert_eq!(percent_encode("a b/é"), "a%20b%2F%C3%A9");
        assert_eq!(percent_decode("%zz%41"), "%zzA");
        let mut out = Vec::new();
        write_response(&mut out, 204, None, b"").unwrap();
        assert!(String::from_utf8(out)
            .unwrap()
            .starts_with("HTTP/1.1 204 No Content\r\n"));
    }
}
