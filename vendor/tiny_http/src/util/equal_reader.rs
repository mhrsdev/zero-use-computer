use std::io::Read;
use std::io::Result as IoResult;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::channel;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

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
    /// Patched (computer-use-mcp): set when the body is left (partly)
    /// unread, so the connection ends instead of reading what is left of it
    /// as its next request.
    left_unread: Arc<AtomicBool>,
}

impl<R> EqualReader<R>
where
    R: Read,
{
    pub fn new(
        reader: R,
        size: usize,
        left_unread: Arc<AtomicBool>,
    ) -> (EqualReader<R>, Receiver<IoResult<()>>) {
        let (tx, rx) = channel();

        let r = EqualReader {
            reader,
            size,
            last_read_signal: tx,
            left_unread,
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
/// next request (as much as computer-use-mcp takes). A longer remainder is
/// left unread, and the connection ends (`left_unread`). The published
/// 0.12.0 allocated a buffer as long as the declared body
/// (`Content-Length`), which a client could set to petabytes and abort the
/// server.
const MAX_DRAIN: usize = 4 * 1024 * 1024;

/// Patched (computer-use-mcp): the longest that is spent reading an unread
/// body (each read also stops at the socket's timeout): a client sending
/// it slowly, or not at all, holds up whoever dropped the reader only so
/// long. What is left is left unread, as above.
const MAX_DRAIN_TIME: Duration = Duration::from_secs(10);

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
            self.left_unread.store(true, Ordering::Relaxed);
            return;
        }

        let mut buf = vec![0; remaining_to_read.min(DRAIN_BUFFER)];
        let started = Instant::now();

        while remaining_to_read > 0 && started.elapsed() < MAX_DRAIN_TIME {
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
        if remaining_to_read > 0 {
            self.left_unread.store(true, Ordering::Relaxed);
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
            let (mut equal_reader, _) =
                EqualReader::new(org_reader.by_ref(), 5, Default::default());

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
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;

        // A body within the limit is read to its end on drop...
        let mut org_reader = Cursor::new(vec![b'x'; 200_000]);
        let left = Arc::new(AtomicBool::new(false));
        drop(EqualReader::new(org_reader.by_ref(), 150_000, left.clone()).0);
        assert_eq!(org_reader.position(), 150_000);
        assert!(!left.load(Ordering::Relaxed));

        // ...a longer one (a declared length far past what was sent) is
        // left, and so is one that ends early: the connection then ends.
        let mut org_reader = Cursor::new(vec![b'x'; 10]);
        let left = Arc::new(AtomicBool::new(false));
        drop(EqualReader::new(org_reader.by_ref(), 1_000_000_000_000_000, left.clone()).0);
        assert_eq!(org_reader.position(), 0);
        assert!(left.load(Ordering::Relaxed));
        let left = Arc::new(AtomicBool::new(false));
        drop(EqualReader::new(org_reader.by_ref(), 20, left.clone()).0);
        assert!(left.load(Ordering::Relaxed));
    }

    #[test]
    fn test_not_enough() {
        use std::io::Cursor;

        let mut org_reader = Cursor::new("hello world".to_string().into_bytes());

        {
            let (mut equal_reader, _) =
                EqualReader::new(org_reader.by_ref(), 5, Default::default());

            let mut vec = [0];
            equal_reader.read_exact(&mut vec).unwrap();
            assert_eq!(vec[0], b'h');
        }

        let mut string = String::new();
        org_reader.read_to_string(&mut string).unwrap();
        assert_eq!(string, " world");
    }
}
