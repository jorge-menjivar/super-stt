// SPDX-License-Identifier: GPL-3.0-only

/// Returned span describes the bytes belonging to ONE SSE block.
/// `start` is the byte index of the blank line, `end` is one past the
/// final `\n` that closes the block.
pub(crate) struct BlankLineBoundary {
    pub(crate) start: usize,
    pub(crate) end: usize,
}

/// Find the boundary between the current SSE block and the next.
/// Per the SSE spec, blocks are separated by a blank line — `\n\n`
/// (LF) or `\r\n\r\n` (CRLF). We accept both.
pub(crate) fn find_blank_line(buffer: &[u8]) -> Option<BlankLineBoundary> {
    let mut i = 0;
    while i + 1 < buffer.len() {
        // \n\n
        if buffer[i] == b'\n' && buffer[i + 1] == b'\n' {
            return Some(BlankLineBoundary {
                start: i,
                end: i + 2,
            });
        }
        // \r\n\r\n
        if i + 3 < buffer.len()
            && buffer[i] == b'\r'
            && buffer[i + 1] == b'\n'
            && buffer[i + 2] == b'\r'
            && buffer[i + 3] == b'\n'
        {
            return Some(BlankLineBoundary {
                start: i,
                end: i + 4,
            });
        }
        i += 1;
    }
    None
}

/// Fields extracted from one SSE block (text before the blank-line boundary).
pub(crate) struct SseFields<'a> {
    pub(crate) event: Option<&'a str>,
    pub(crate) data: String,
    /// True if the block contained at least one `:` comment line.
    pub(crate) saw_comment: bool,
}

/// Parse the `event:` / `data:` / comment lines out of one SSE block.
/// Per spec, multiple `data:` lines concatenate with `\n`; `id:`/`retry:`
/// are ignored.
pub(crate) fn parse_fields(block: &str) -> SseFields<'_> {
    let mut event: Option<&str> = None;
    let mut data = String::new();
    let mut saw_comment = false;
    for raw_line in block.split('\n') {
        let line = raw_line.trim_end_matches('\r');
        if line.is_empty() {
            continue;
        }
        if line.starts_with(':') {
            saw_comment = true;
            continue;
        }
        if let Some(rest) = line.strip_prefix("event:") {
            event = Some(rest.trim_start());
        } else if let Some(rest) = line.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(rest.trim_start());
        }
    }
    SseFields {
        event,
        data,
        saw_comment,
    }
}
