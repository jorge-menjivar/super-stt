// SPDX-License-Identifier: GPL-3.0-only
//! Write the daemon protocol's `OpenAPI` document to `docs/protocol/openapi.json`.
//!
//! Run it with `just openapi` after changing anything under
//! `src/daemon/http/v1/`; `just openapi-check` (part of `just ci`) fails when
//! the committed file no longer matches what the router produces, so the
//! published spec cannot fall behind the protocol.
//!
//! The document is built from the route registrations themselves, so this
//! starts no daemon, opens no socket, and touches no keyring.

use std::io::Write;
use std::path::{Path, PathBuf};

/// Resolve `docs/protocol/openapi.json` from the crate's own location, so the
/// generator writes the same file whatever directory it is invoked from.
fn output_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the daemon crate always sits inside the workspace root")
        .join("docs/protocol/openapi.json")
}

fn main() -> std::io::Result<()> {
    let doc = super_stt_daemon::daemon::http::openapi_document();
    let mut json = doc
        .to_pretty_json()
        .expect("the generated document is always serializable");
    // A trailing newline, so the committed file is a well-formed text file and
    // `git diff` does not report "\ No newline at end of file" on every change.
    json.push('\n');

    let path = output_path();
    let mut file = std::fs::File::create(&path)?;
    file.write_all(json.as_bytes())?;

    let paths = doc.paths.paths.len();
    println!("wrote {} ({paths} paths)", path.display());
    Ok(())
}
