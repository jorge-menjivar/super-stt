// SPDX-License-Identifier: GPL-3.0-only
use super::*;

#[test]
fn test_preprocess_text() {
    // Basic functionality
    assert_eq!(preprocess_text("hello world", true), "Hello world");
    assert_eq!(preprocess_text("hello world", false), "Hello world.");
    assert_eq!(preprocess_text("", true), "");

    assert_eq!(preprocess_text("...hello world", true), "Hello world");
    assert_eq!(preprocess_text("  ...  hello world  ", true), "Hello world");
    assert_eq!(
        preprocess_text("  multiple   spaces  ", true),
        "Multiple spaces"
    );
}

#[test]
fn test_is_simple_extension() {
    assert!(is_simple_extension("hello", "hello world"));
    assert!(is_simple_extension("", "hello"));
    assert!(!is_simple_extension("hello", "hi world"));
    assert!(!is_simple_extension("hello", "hello"));
    assert!(!is_simple_extension("hello world", "hello"));
}

#[test]
fn test_find_tail_match_in_text() {
    // Test the key case: "engi" should match with "engineer"
    assert_eq!(
        find_tail_match_in_text("hello engi", "engineer is good", 4),
        4
    );

    // Test basic tail matching
    assert_eq!(
        find_tail_match_in_text("hello world", "world is nice", 5),
        5
    );

    // Test no match
    assert_eq!(find_tail_match_in_text("hello", "goodbye", 3), -1);

    // Test short strings
    assert_eq!(find_tail_match_in_text("hi", "hello", 3), -1);

    // Test exact match at end
    assert_eq!(find_tail_match_in_text("abc", "xyzabc", 3), 6);
}

#[test]
fn find_tail_match_returns_utf8_safe_byte_offset() {
    // "xyzáb" holds a 2-byte 'á' before the match boundary, so the matched-tail
    // position as a CHAR index (4) differs from the BYTE offset (5). The result
    // must be a byte offset: callers slice `&text2[pos..]`, and a char index
    // would land inside the multibyte 'á' and panic ("not a char boundary").
    let pos = find_tail_match_in_text("wyzá", "xyzáb", 3);
    assert_eq!(
        pos, 5,
        "expected the byte offset (5), not the char index (4)"
    );
    // Must be usable as a &str byte slice without panicking.
    let suffix = &"xyzáb"[usize::try_from(pos).unwrap()..];
    assert_eq!(suffix, "b");
}

#[test]
fn test_find_common_prefix() {
    assert_eq!(find_common_prefix("hello world", "hello there"), 6);
    assert_eq!(find_common_prefix("abc", "def"), 0);
    assert_eq!(find_common_prefix("same text", "same text"), 9);
}
