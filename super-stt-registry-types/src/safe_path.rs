// SPDX-License-Identifier: GPL-3.0-only

/// A backend `id` must be a single, relative, non-traversing path component
/// before it is used in a `Path::join`. Reject empty, `.`, `..`, anything
/// containing a path separator, and embedded NUL — these are the inputs that
/// would let a `join` escape the backends directory or select an absolute host
/// path.
#[must_use]
pub fn is_safe_component(s: &str) -> bool {
    !s.is_empty()
        && s != "."
        && s != ".."
        && !s.contains('/')
        && !s.contains('\\')
        && !s.contains('\0')
}

/// A backend `entrypoint` may be a nested relative path (e.g. `bin/launcher`)
/// so a multi-file bundle can keep its executable under a subdirectory. Like
/// [`is_safe_component`] it is joined onto the backend directory, so it must
/// not escape it: reject empty, absolute paths, any empty / `.` / `..`
/// component, backslashes, and embedded NUL. A single safe component (the
/// common case, e.g. a self-contained binary) also satisfies this.
#[must_use]
pub fn is_safe_relative_path(s: &str) -> bool {
    if s.is_empty() || s.starts_with('/') || s.contains('\\') || s.contains('\0') {
        return false;
    }
    let mut saw_component = false;
    for component in s.split('/') {
        if component.is_empty() || component == "." || component == ".." {
            return false;
        }
        saw_component = true;
    }
    saw_component
}

#[cfg(test)]
mod tests {
    use super::{is_safe_component, is_safe_relative_path};

    #[test]
    fn accepts_plain_components() {
        assert!(is_safe_component("openai.wasm"));
        assert!(is_safe_component("super-stt-backend-whisper"));
        assert!(is_safe_component("mistral"));
    }

    #[test]
    fn rejects_traversal_and_separators() {
        assert!(!is_safe_component(""));
        assert!(!is_safe_component("."));
        assert!(!is_safe_component(".."));
        assert!(!is_safe_component("../evil"));
        assert!(!is_safe_component("a/b"));
        assert!(!is_safe_component("/usr/bin/python3"));
        assert!(!is_safe_component("a\\b"));
        assert!(!is_safe_component("a\0b"));
    }

    #[test]
    fn relative_path_accepts_components_and_nested() {
        assert!(is_safe_relative_path("super-stt-backend-voxtral"));
        assert!(is_safe_relative_path("bin/qwen3-asr"));
        assert!(is_safe_relative_path("bin/sub/exec"));
    }

    #[test]
    fn relative_path_rejects_traversal_and_absolute() {
        for bad in [
            "",
            "/abs",
            "../evil",
            "bin/../../evil",
            "a/./b",
            "a//b",
            "bin/",
            ".",
            "..",
            "a\\b",
            "a\0b",
        ] {
            assert!(!is_safe_relative_path(bad), "{bad:?} must be rejected");
        }
    }
}
