//! Bounded Content-Length framing; stdout contains JSON-RPC messages only.

use std::io::{self, BufRead, Read, Write};

use serde_json::Value;

const MAX_HEADER: u64 = 8 * 1024;
const MAX_BODY: usize = 16 * 1024 * 1024;

pub(super) fn read(reader: &mut impl BufRead) -> io::Result<Option<Vec<u8>>> {
    let mut length = None;
    let mut total = 0;
    loop {
        let mut line = String::new();
        let count = reader.take(MAX_HEADER + 1).read_line(&mut line)?;
        if count == 0 && total == 0 {
            return Ok(None);
        }
        total += count as u64;
        if total > MAX_HEADER || !line.ends_with("\r\n") {
            return Err(invalid("Invalid or oversized LSP header"));
        }
        if line == "\r\n" {
            break;
        }
        let (name, value) = line
            .trim_end()
            .split_once(':')
            .ok_or_else(|| invalid("Invalid LSP header field"))?;
        if name.eq_ignore_ascii_case("Content-Length") {
            if length.is_some() {
                return Err(invalid("Duplicate Content-Length header"));
            }
            let value: usize = value
                .trim()
                .parse()
                .map_err(|_| invalid("Invalid Content-Length"))?;
            if value > MAX_BODY {
                return Err(invalid("LSP message exceeds 16 MiB"));
            }
            length = Some(value);
        }
    }
    let mut body = vec![0; length.ok_or_else(|| invalid("Missing Content-Length header"))?];
    reader.read_exact(&mut body)?;
    Ok(Some(body))
}

pub(super) fn write(writer: &mut impl Write, message: &Value) -> io::Result<()> {
    let body = serde_json::to_vec(message)?;
    write!(writer, "Content-Length: {}\r\n\r\n", body.len())?;
    writer.write_all(&body)?;
    writer.flush()
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn frames_use_utf8_byte_lengths_and_round_trip_consecutive_messages() {
        let mut bytes = Vec::new();
        for value in [json!({"text":"中文🦀"}), json!({"id":2})] {
            write(&mut bytes, &value).unwrap();
        }
        let mut input = bytes.as_slice();
        assert_eq!(
            serde_json::from_slice::<Value>(&read(&mut input).unwrap().unwrap()).unwrap(),
            json!({"text":"中文🦀"})
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&read(&mut input).unwrap().unwrap()).unwrap(),
            json!({"id":2})
        );
        assert!(read(&mut input).unwrap().is_none());
    }

    #[test]
    fn malformed_truncated_and_oversized_frames_fail_without_allocating_the_body() {
        for text in [
            "Content-Length: 16777217\r\n\r\n",
            "Content-Length: -1\r\n\r\n",
            "Content-Length: 1\r\nContent-Length: 1\r\n\r\nx",
            "Content-Type: application/json\r\n\r\n{}",
            "Content-Length: 3\r\n\r\n{}",
            "Content-Length: 3\n\n{}",
            "Content-Length: 3",
        ] {
            assert!(read(&mut text.as_bytes()).is_err(), "{text:?}");
        }
        assert!(read(&mut "x".repeat(9000).as_bytes()).is_err());
    }

    #[test]
    fn accepts_case_insensitive_headers_and_content_type() {
        let text = "content-length: 2\r\nContent-Type: application/vscode-jsonrpc; charset=utf-8\r\n\r\n{}";
        assert_eq!(read(&mut text.as_bytes()).unwrap(), Some(b"{}".to_vec()));
    }
}
