# tiny_http 0.12.0, patched for computer-use-mcp

Copied from crates.io (MIT or Apache-2.0, licences beside this file) and used
through `[patch.crates-io]` in the workspace `Cargo.toml`. Tests, examples and
benches were left out. Changes, each marked "Patched" in the code:

- `src/util/equal_reader.rs`: an unread body is drained through a small
  buffer and only up to 1 MiB, instead of one buffer the size the client
  declared (a huge `Content-Length` aborted the process).
- `src/client.rs`: a request or header line longer than 16 KiB, or more than
  256 header lines, ends the connection instead of growing in memory.
- `src/lib.rs`: two warnings the published code gives on today's compilers are allowed.
