// SPDX-License-Identifier: GPL-3.0-only
use super::*;

#[test]
fn test_preprocess_text() {
    // Basic functionality
    assert_eq!(Typer::preprocess_text("hello world", true), "Hello world");
    assert_eq!(Typer::preprocess_text("hello world", false), "Hello world.");
    assert_eq!(Typer::preprocess_text("", true), "");

    assert_eq!(
        Typer::preprocess_text("...hello world", true),
        "Hello world"
    );
    assert_eq!(
        Typer::preprocess_text("  ...  hello world  ", true),
        "Hello world"
    );
    assert_eq!(
        Typer::preprocess_text("  multiple   spaces  ", true),
        "Multiple spaces"
    );
}

#[test]
fn test_is_simple_extension() {
    assert!(Typer::is_simple_extension("hello", "hello world"));
    assert!(Typer::is_simple_extension("", "hello"));
    assert!(!Typer::is_simple_extension("hello", "hi world"));
    assert!(!Typer::is_simple_extension("hello", "hello"));
    assert!(!Typer::is_simple_extension("hello world", "hello"));
}

#[test]
fn test_find_tail_match_in_text() {
    // Test the key case: "engi" should match with "engineer"
    assert_eq!(
        Typer::find_tail_match_in_text("hello engi", "engineer is good", 4),
        4
    );

    // Test basic tail matching
    assert_eq!(
        Typer::find_tail_match_in_text("hello world", "world is nice", 5),
        5
    );

    // Test no match
    assert_eq!(Typer::find_tail_match_in_text("hello", "goodbye", 3), -1);

    // Test short strings
    assert_eq!(Typer::find_tail_match_in_text("hi", "hello", 3), -1);

    // Test exact match at end
    assert_eq!(Typer::find_tail_match_in_text("abc", "xyzabc", 3), 6);
}

#[test]
fn test_find_common_prefix() {
    assert_eq!(Typer::find_common_prefix("hello world", "hello there"), 6);
    assert_eq!(Typer::find_common_prefix("abc", "def"), 0);
    assert_eq!(Typer::find_common_prefix("same text", "same text"), 9);
}
