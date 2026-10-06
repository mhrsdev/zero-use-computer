use std::io::Read;
use std::io::Result as IoResult;
use std::sync::mpsc::channel;
use std::sync::mpsc::{Receiver, Sender};

/// A `Reader` that reads exactly the number of bytes from a sub-reader.
///
/// If the limit is reached, it returns EOF. If the limit is not reached
/// when the destructor is called, the remaining bytes will be read and
/// thrown away.
pub struct EqualReader<R>
where
    R: Read,
{
    reader: R,
    size: usize,
    last_read_signal: Sender<IoResult<()>>,
}

impl<R> EqualReader<R>
where
    R: Read,
{
    pub fn new(reader: R, size: usize) -> (EqualReader<R>, Receiver<IoResult<()>>) {
        let (tx, rx) = channel();

        let r = EqualReader {
            reader,
            size,
            last_read_signal: tx,
        };

        (r, rx)
    }
}

impl<R> Read for EqualReader<R>
where
    R: Read,
{
    fn read(&mut self, buf: &mut [u8]) -> IoResult<usize> {
        if self.size == 0 {
            return Ok(0);
        }

        let buf = if buf.len() < self.size {
            buf
        } else {
            &mut buf[..self.size]
        };

        match self.reader.read(buf) {
            Ok(len) => {
                self.size -= len;
                Ok(len)
            }
            err @ Err(_) => err,
        }
    }
}

/// Patched (computer-use-mcp): the most of an unread body that is read and
/// thrown away when the reader is dropped, so the connection can serve its
/// next request. A longer remainder is left unread, as an unread chunked
/// body already is: it is then read as the connection's next request,
/// which fails to parse, and the connection is closed. The published 0.12.0 allocated a
/// buffer as long as the declared body (`Content-Length`), which a client
/// could set to petabytes and abort the server.
const MAX_DRAIN: usize = 1024 * 1024;

/// Patched (computer-use-mcp): the body is drained through a buffer of at
/// most this size.
const DRAIN_BUFFER: usize = 64 * 1024;

impl<R> Drop for EqualReader<R>
where
    R: Read,
{
    fn drop(&mut self) {
        let mut remaining_to_read = self.size;
        if remaining_to_read > MAX_DRAIN {
            return;
        }

        let mut buf = vec![0; remaining_to_read.min(DRAIN_BUFFER)];

        while remaining_to_read > 0 {
            let len = remaining_to_read.min(buf.len());

            match self.reader.read(&mut buf[..len]) {
                Err(e) => {
                    self.last_read_signal.send(Err(e)).ok();
                    break;
                }
                Ok(0) => {
                    self.last_read_signal.send(Ok(())).ok();
                    break;
                }
                Ok(other) => {
                    remaining_to_read -= other;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::EqualReader;
    use std::io::Read;

    #[test]
    fn test_limit() {
        use std::io::Cursor;

        let mut org_reader = Cursor::new("hello world".to_string().into_bytes());

        {
            let (mut equal_reader, _) = EqualReader::new(org_reader.by_ref(), 5);

            let mut string = String::new();
            equal_reader.read_to_string(&mut string).unwrap();
            assert_eq!(string, "hello");
        }

        let mut string = String::new();
        org_reader.read_to_string(&mut string).unwrap();
        assert_eq!(string, " world");
    }

    #[test]
    fn test_drain_is_capped() {
        use std::io::Cursor;

        // A body within the limit is read to its end on drop...
        let mut org_reader = Cursor::new(vec![b'x'; 200_000]);
        drop(EqualReader::new(org_reader.by_ref(), 150_000).0);
        assert_eq!(org_reader.position(), 150_000);

        // ...a longer one (a declared length far past what was sent) is left.
        let mut org_reader = Cursor::new(vec![b'x'; 10]);
        drop(EqualReader::new(org_reader.by_ref(), 1_000_000_000_000_000).0);
        assert_eq!(org_reader.position(), 0);
    }

    #[test]
    fn test_not_enough() {
        use std::io::Cursor;

        let mut org_reader = Cursor::new("hello world".to_string().into_bytes());

        {
            let (mut equal_reader, _) = EqualReader::new(org_reader.by_ref(), 5);

            let mut vec = [0];
            equal_reader.read_exact(&mut vec).unwrap();
            assert_eq!(vec[0], b'h');
        }

        let mut string = String::new();
        org_reader.read_to_string(&mut string).unwrap();
        assert_eq!(string, " world");
    }
}
