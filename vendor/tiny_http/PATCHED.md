# tiny_http 0.12.0, patched for computer-use-mcp

Copied from crates.io (MIT or Apache-2.0, licences beside this file) and used
through `[patch.crates-io]` in the workspace `Cargo.toml`. Tests, examples and
benches were left out. Changes, each marked "Patched" in the code:

- `src/util/equal_reader.rs`: an unread body is drained through a small
  buffer, only up to 4 MiB and for at most 10 seconds, instead of through one
  buffer the size the client declared (a huge `Content-Length` aborted the
  process) for as long as the client takes. A body left (partly) unread sets
  a flag of its connection (passed in through `src/request.rs`, and
  `src/test.rs` for test requests).
- `src/client.rs`: a request or header line longer than 16 KiB, or more than
  256 header lines, ends the connection instead of growing in memory. So
  does a body left unread (the flag above): what follows it was read as the
  next request, which a client could fill with one of its own.
- `src/lib.rs` (with `src/connection.rs`): every connection gets a 10-second
  read timeout, so a client that stops sending (a request's head, or a body
  being drained) no longer holds a thread for good. The wait for a request
  to begin is not limited (`src/client.rs`): a keep-alive connection, or one
  whose answer takes long, is not closed under the client. Two warnings the published code gives on today's
  compilers are allowed.
