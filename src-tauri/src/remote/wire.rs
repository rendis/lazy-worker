//! The lines that the helper and the app socket exchange.
//!
//! Every message is one JSON object on one line. A reader never holds more than its limit.

use std::io::{self, BufRead};

/// The longest request line that the helper or the app accepts.
pub const MAX_REQUEST_LINE: usize = 64 * 1024;
/// The longest response line that the helper relays. A log of 1000 entries stays well under it.
pub const MAX_RESPONSE_LINE: usize = 1024 * 1024;

#[derive(Debug)]
pub enum LineError {
    TooLong,
    Io,
}

/// Reads one line without its end mark. Returns `None` at the end of the input.
/// A line over `max` bytes is an error, and the reader stops after at most `max` + one buffer of it.
pub fn read_line(reader: &mut impl BufRead, max: usize) -> Result<Option<Vec<u8>>, LineError> {
    let mut line = Vec::new();
    loop {
        let buffer = match reader.fill_buf() {
            Ok(buffer) => buffer,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => return Err(LineError::Io),
        };
        if buffer.is_empty() {
            return Ok((!line.is_empty()).then_some(line));
        }
        let (take, found) = match buffer.iter().position(|&byte| byte == b'\n') {
            Some(index) => (index, true),
            None => (buffer.len(), false),
        };
        if line.len() + take > max {
            return Err(LineError::TooLong);
        }
        line.extend_from_slice(&buffer[..take]);
        reader.consume(take + usize::from(found));
        if found {
            return Ok(Some(line));
        }
    }
}

/// `{"error":"<code>"}` with its end mark. Codes are fixed words, never text from outside.
pub fn error_line(code: &str) -> String {
    format!("{}\n", serde_json::json!({ "error": code }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn lines_are_read_one_at_a_time_and_the_last_may_lack_its_end_mark() {
        let mut input = Cursor::new(b"one\ntwo\nthree".to_vec());
        assert_eq!(read_line(&mut input, 16).unwrap().unwrap(), b"one");
        assert_eq!(read_line(&mut input, 16).unwrap().unwrap(), b"two");
        assert_eq!(read_line(&mut input, 16).unwrap().unwrap(), b"three");
        assert!(read_line(&mut input, 16).unwrap().is_none());
    }

    #[test]
    fn a_line_at_the_limit_passes_and_one_byte_more_does_not() {
        let mut ok = vec![b'a'; 8];
        ok.push(b'\n');
        assert_eq!(
            read_line(&mut Cursor::new(ok), 8).unwrap().unwrap().len(),
            8
        );
        let mut long = vec![b'a'; 9];
        long.push(b'\n');
        assert!(matches!(
            read_line(&mut Cursor::new(long), 8),
            Err(LineError::TooLong)
        ));
        // No end mark at all still stops at the limit.
        assert!(matches!(
            read_line(&mut Cursor::new(vec![b'a'; 100]), 8),
            Err(LineError::TooLong)
        ));
    }

    #[test]
    fn error_lines_are_json() {
        assert_eq!(error_line("notPaired"), "{\"error\":\"notPaired\"}\n");
    }
}
