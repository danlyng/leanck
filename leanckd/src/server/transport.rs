//! LSP message framing: `Content-Length` headers over a byte stream.

use std::io;

use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt};

/// Read one LSP message body (the JSON after the headers). Returns `Ok(None)` at
/// clean EOF. Only `Content-Length` is honored; other headers are ignored.
pub async fn read_message<R: AsyncBufRead + Unpin>(reader: &mut R) -> io::Result<Option<Vec<u8>>> {
    let mut content_length: Option<usize> = None;
    let mut line = Vec::new();
    loop {
        line.clear();
        let n = reader.read_until(b'\n', &mut line).await?;
        if n == 0 {
            // EOF. Mid-header EOF is still treated as a clean close.
            return Ok(None);
        }
        let trimmed = trim_crlf(&line);
        if trimmed.is_empty() {
            break; // blank line terminates the header block
        }
        if let Some((key, value)) = split_header(trimmed) {
            if key.eq_ignore_ascii_case(b"content-length") {
                let s = std::str::from_utf8(value)
                    .map_err(|_| invalid("Content-Length not UTF-8"))?
                    .trim();
                content_length =
                    Some(s.parse().map_err(|_| invalid("Content-Length not a number"))?);
            }
        }
    }
    let len = content_length.ok_or_else(|| invalid("missing Content-Length header"))?;
    let mut body = vec![0u8; len];
    reader.read_exact(&mut body).await?;
    Ok(Some(body))
}

/// Frame a message body with an LSP `Content-Length` header.
pub fn encode_message(body: &[u8]) -> Vec<u8> {
    let header = format!("Content-Length: {}\r\n\r\n", body.len());
    let mut out = Vec::with_capacity(header.len() + body.len());
    out.extend_from_slice(header.as_bytes());
    out.extend_from_slice(body);
    out
}

fn trim_crlf(line: &[u8]) -> &[u8] {
    let mut end = line.len();
    while end > 0 && (line[end - 1] == b'\n' || line[end - 1] == b'\r') {
        end -= 1;
    }
    &line[..end]
}

fn split_header(line: &[u8]) -> Option<(&[u8], &[u8])> {
    let colon = line.iter().position(|&b| b == b':')?;
    Some((&line[..colon], &line[colon + 1..]))
}

fn invalid(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[tokio::test]
    async fn round_trips_a_message() {
        let body = br#"{"jsonrpc":"2.0","id":1,"result":null}"#;
        let framed = encode_message(body);
        let mut cur = Cursor::new(framed);
        let got = read_message(&mut cur).await.unwrap().unwrap();
        assert_eq!(got, body);
    }

    #[tokio::test]
    async fn reads_two_messages_back_to_back() {
        let mut buf = encode_message(b"{\"a\":1}");
        buf.extend(encode_message(b"{\"b\":2}"));
        let mut cur = Cursor::new(buf);
        assert_eq!(read_message(&mut cur).await.unwrap().unwrap(), b"{\"a\":1}");
        assert_eq!(read_message(&mut cur).await.unwrap().unwrap(), b"{\"b\":2}");
        assert!(read_message(&mut cur).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn tolerates_extra_headers_and_lowercase_key() {
        let body = b"{\"x\":true}";
        let msg = format!(
            "Content-Type: application/json\r\ncontent-length: {}\r\n\r\n",
            body.len()
        );
        let mut buf = msg.into_bytes();
        buf.extend_from_slice(body);
        let mut cur = Cursor::new(buf);
        assert_eq!(read_message(&mut cur).await.unwrap().unwrap(), body);
    }

    #[tokio::test]
    async fn clean_eof_returns_none() {
        let mut cur = Cursor::new(Vec::new());
        assert!(read_message(&mut cur).await.unwrap().is_none());
    }
}
