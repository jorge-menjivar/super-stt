// SPDX-License-Identifier: GPL-3.0-only
use super::{Normalizer, normalize};

#[test]
fn plain_text_passes_through_with_whitespace_collapsed() {
    assert_eq!(
        normalize("Hello   there.  How\tare you?"),
        "Hello there. How are you?"
    );
    // A blank line is a paragraph break, kept as one newline.
    assert_eq!(
        normalize("Hello   there.\n\nHow are you?"),
        "Hello there.\nHow are you?"
    );
    assert_eq!(
        normalize("  leading and trailing  "),
        "leading and trailing"
    );
}

#[test]
fn emphasis_markers_are_not_spoken() {
    assert_eq!(
        normalize("This is **bold** and *italic*."),
        "This is bold and italic."
    );
    assert_eq!(normalize("snake_case_word"), "snakecaseword");
    assert_eq!(normalize("~~struck~~ out"), "struck out");
}

#[test]
fn inline_code_keeps_its_contents_but_not_its_backticks() {
    assert_eq!(normalize("Run `cargo test` now."), "Run cargo test now.");
}

#[test]
fn a_fenced_code_block_becomes_a_short_notice() {
    let md = "Before\n```rust\nfn main() {\n    println!(\"hi\");\n}\n```\nAfter";
    assert_eq!(normalize(md), "Before\ncode block\nAfter");
}

/// An unterminated fence must not swallow the rest of the utterance — the
/// listener should still learn a code block was there.
#[test]
fn an_unclosed_fence_still_resolves_at_the_end() {
    assert_eq!(normalize("Before\n```\nfn main() {"), "Before\ncode block");
}

#[test]
fn headings_bullets_and_quotes_lose_their_markers() {
    assert_eq!(normalize("# Title\nBody"), "Title\nBody");
    assert_eq!(normalize("### Deep\ntext"), "Deep\ntext");
    assert_eq!(normalize("- one\n- two"), "one\ntwo");
    assert_eq!(normalize("> quoted"), "quoted");
}

/// A heading, a list item, a paragraph: each ends where its line does, period
/// or no period, and the model must not run them together. The line break is
/// kept as `\n` for the chunker whenever the page says the line ended.
#[test]
fn a_line_break_that_ends_a_line_is_kept() {
    // The page says so: a blank line, a heading on either side, a list item,
    // a quote, a fence.
    assert_eq!(
        normalize("First paragraph\n\nSecond"),
        "First paragraph\nSecond"
    );
    assert_eq!(normalize("## Summary\nall good"), "Summary\nall good");
    assert_eq!(normalize("Intro\n## Next"), "Intro\nNext");
    assert_eq!(normalize("- alpha\n- beta\n- gamma"), "alpha\nbeta\ngamma");
    assert_eq!(
        normalize("1. one\n2. two\n10) ten"),
        "1. one\n2. two\n10) ten"
    );
    assert_eq!(normalize("Note\n> quoted"), "Note\nquoted");
    assert_eq!(
        normalize("Here:\n```\nx\n```\nDone"),
        "Here:\ncode block\nDone"
    );
    // Plain lines that start the way a sentence does.
    assert_eq!(
        normalize("Summary\nEverything passed\nNext steps"),
        "Summary\nEverything passed\nNext steps"
    );
    assert_eq!(
        normalize("Step one\n\"Quoted\" line"),
        "Step one\n\"Quoted\" line"
    );
    assert_eq!(normalize("Step one\n**Bold** line"), "Step one\nBold line");
    assert_eq!(normalize("Total\n42 items"), "Total\n42 items");
    // A script without case: every line ends.
    assert_eq!(normalize("状態\n正常"), "状態\n正常");
    // Windows line endings are line endings.
    assert_eq!(normalize("One\r\nTwo"), "One\nTwo");
}

/// The other thing a newline means: hard-wrapped prose, where the line ends
/// mid-sentence. The next line starting lowercase, or the line before ending
/// mid-clause, says the sentence goes on.
#[test]
fn a_line_break_that_wraps_a_sentence_is_a_space() {
    assert_eq!(
        normalize("The quick brown fox jumps over\nthe lazy dog."),
        "The quick brown fox jumps over the lazy dog."
    );
    assert_eq!(
        normalize("apples, pears,\nAnd plums"),
        "apples, pears, And plums"
    );
    assert_eq!(normalize("first;\nSecond"), "first; Second");
    // A lazy continuation of a list item.
    assert_eq!(
        normalize("- an item that wraps\nonto a second line"),
        "an item that wraps onto a second line"
    );
    // Opening markup is looked through to the first letter.
    assert_eq!(
        normalize("*emphasis* opens\n*this* line"),
        "emphasis opens this line"
    );
    assert_eq!(
        normalize("see the\n[docs](https://x.com) here"),
        "see the docs here"
    );
    // A bare `-` or digit that is not a marker is judged as text.
    assert_eq!(normalize("offset\n-3 units"), "offset\n-3 units");
    assert_eq!(normalize("version\n1.5 is out"), "version\n1.5 is out");
}

/// A hyphen only means "bullet" at the start of a line followed by a space.
/// Mid-sentence it is a real character that must survive.
#[test]
fn a_hyphen_is_only_a_bullet_at_the_start_of_a_line() {
    assert_eq!(normalize("well-known result"), "well-known result");
    assert_eq!(normalize("a -3 offset"), "a -3 offset");
}

#[test]
fn links_speak_their_label_not_their_target() {
    assert_eq!(
        normalize("See [the docs](https://example.com/a/b?c=d) for more."),
        "See the docs for more."
    );
    assert_eq!(normalize("A [bare] bracket"), "A bare bracket");
}

#[test]
fn a_bare_url_is_reduced_to_its_host() {
    assert_eq!(
        normalize("Go to https://example.com/very/long/path?q=1 now"),
        "Go to example.com now"
    );
    assert_eq!(normalize("visit www.example.org."), "visit www.example.org");
}

#[test]
fn emoji_and_table_pipes_are_dropped() {
    assert_eq!(normalize("Done ✅ shipped 🚀"), "Done shipped");
    assert_eq!(normalize("| a | b |"), "a b");
}

/// Numbers, dates, and currency are the model's job, not the normalizer's — a
/// wrong number-to-words is worse than none, and every model in scope reads
/// these correctly.
#[test]
fn numbers_dates_and_currency_are_left_exactly_as_written() {
    for s in [
        "It cost $1,234.56 today.",
        "Released 2026-08-29 at 14:30.",
        "Up 12.5% from 3.14 to 3.5.",
        "Call +1 (555) 010-9999.",
    ] {
        assert_eq!(normalize(s), s, "{s} must survive untouched");
    }
}

#[test]
fn non_latin_text_is_not_mangled() {
    assert_eq!(normalize("こんにちは、世界。"), "こんにちは、世界。");
    assert_eq!(normalize("Привет, мир!"), "Привет, мир!");
    assert_eq!(normalize("**混合** text"), "混合 text");
}

/// The streaming contract: feeding one character at a time must produce
/// exactly what feeding the whole string produces. This is what catches a
/// normalizer that decides on markup before it has enough input.
#[test]
fn streaming_a_delta_at_a_time_matches_the_whole_string() {
    let cases = [
        "This is **bold** and *italic* text.",
        "Run `cargo test` and see [docs](https://x.com/y).",
        "# Title\n- one\n- two\nGo to https://example.com/path now.",
        "Before\n```rust\nfn main() {}\n```\nAfter",
        "Plain sentence with 3.14 and $10.",
        "Summary\nEverything passed\nNext steps",
        "1. one\n2. two\n\nwrapped over\nthe line, and\nAnother",
        "> q\n\npara\n- item\n*this* wraps\n10) ten",
    ];
    for case in cases {
        let whole = normalize(case);
        let mut n = Normalizer::new();
        let mut streamed = String::new();
        for c in case.chars() {
            streamed.push_str(&n.push(&c.to_string()));
        }
        streamed.push_str(&n.finish());
        assert_eq!(
            streamed.trim(),
            whole,
            "streaming {case:?} must match the one-shot result"
        );
    }
}

/// The specific ambiguity the streaming design exists for: a trailing `*` could
/// open emphasis or be a literal, and committing early speaks an asterisk.
#[test]
fn an_ambiguous_trailing_marker_is_held_rather_than_guessed() {
    let mut n = Normalizer::new();
    let out = n.push("bold *");
    assert!(
        !out.contains('*'),
        "a dangling marker must not be emitted as text, got {out:?}"
    );
    let more = n.push("word* done");
    assert!(
        !more.contains('*'),
        "and must still not be, once it resolves to emphasis: {more:?}"
    );
    let all = format!("{out}{more}{}", n.finish());
    assert_eq!(all.trim(), "bold word done");
}

/// A partial fence must not be mistaken for inline code before the third
/// backtick arrives.
#[test]
fn a_partial_fence_is_not_committed_as_inline_code() {
    let mut n = Normalizer::new();
    let mut out = n.push("text ``");
    out.push_str(&n.push("`\ncode here\n```\nafter"));
    out.push_str(&n.finish());
    assert_eq!(out.trim(), "text\ncode block\nafter");
}

/// Without a cap an unterminated construct would hold forever and nothing would
/// ever be spoken.
#[test]
fn an_unbounded_held_tail_is_eventually_forced_out() {
    let mut n = Normalizer::new();
    // One backtick then a very long run of ordinary text with no close.
    let out = n.push(&format!("`{}", "a".repeat(super::MAX_HELD + 100)));
    assert!(
        !out.is_empty(),
        "past the cap the normalizer must emit rather than hold forever"
    );
}

#[test]
fn an_empty_input_produces_nothing() {
    assert_eq!(normalize(""), "");
    assert_eq!(normalize("   \n\t "), "");
    let mut n = Normalizer::new();
    assert_eq!(n.push(""), "");
    assert_eq!(n.finish(), "");
}

/// A normalizer is reused across utterances, so `finish` has to leave it clean.
#[test]
fn finish_resets_state_for_the_next_utterance() {
    let mut n = Normalizer::new();
    let _ = n.push("```\ncode");
    let _ = n.finish();
    let mut out = n.push("plain text");
    out.push_str(&n.finish());
    assert_eq!(out.trim(), "plain text");
}
