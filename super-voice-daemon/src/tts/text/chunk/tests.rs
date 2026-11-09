// SPDX-License-Identifier: GPL-3.0-only
use super::{ChunkPolicy, Chunker};

/// No short opener and no limit: the simplest policy, for tests about
/// boundary detection rather than sizing.
fn plain() -> ChunkPolicy {
    ChunkPolicy {
        max_chars: None,
        first_chunk_chars: 0,
    }
}

fn chunk_all(policy: ChunkPolicy, text: &str) -> Vec<String> {
    let mut c = Chunker::new(policy);
    let mut out = c.push(text);
    out.extend(c.finish());
    out
}

#[test]
fn text_within_the_limit_is_one_chunk() {
    assert_eq!(
        chunk_all(plain(), "Hello there. How are you?"),
        vec!["Hello there. How are you?"]
    );
}

#[test]
fn an_empty_input_produces_no_chunks() {
    assert!(chunk_all(plain(), "").is_empty());
    assert!(chunk_all(plain(), "   ").is_empty());
}

/// The short opener exists to start speech quickly; everything after it packs
/// as full as the limit allows.
#[test]
fn the_first_chunk_is_short_and_the_rest_are_not() {
    let policy = ChunkPolicy {
        max_chars: None,
        first_chunk_chars: 30,
    };
    let text = "Short opener here. Then a considerably longer stretch of text that would \
                otherwise have been merged into the very first chunk and delayed it.";
    let chunks = chunk_all(policy, text);
    assert_eq!(chunks.len(), 2, "one opener plus the remainder: {chunks:?}");
    assert_eq!(chunks[0], "Short opener here.");
    assert!(
        chunks[1].len() > 60,
        "the rest is not split further: {:?}",
        chunks[1]
    );
}

#[test]
fn chunks_never_exceed_the_models_limit() {
    let policy = ChunkPolicy {
        max_chars: Some(40),
        first_chunk_chars: 0,
    };
    let text = "One sentence here. Another sentence follows. And a third one arrives. \
                Finally a fourth.";
    for chunk in chunk_all(policy, text) {
        assert!(
            chunk.chars().count() <= 40,
            "chunk over the limit: {chunk:?}"
        );
    }
}

/// A single sentence longer than the limit still has to be spoken, so the
/// chunker falls back to a word break rather than refusing.
#[test]
fn an_oversized_sentence_falls_back_to_a_word_break() {
    let policy = ChunkPolicy {
        max_chars: Some(30),
        first_chunk_chars: 0,
    };
    let text = "this one very long sentence has no terminator anywhere inside it at all";
    let chunks = chunk_all(policy, text);
    assert!(chunks.len() > 1);
    for chunk in &chunks {
        assert!(chunk.chars().count() <= 30, "{chunk:?}");
        assert!(
            !chunk.starts_with(' ') && !chunk.ends_with(' '),
            "chunks are trimmed: {chunk:?}"
        );
    }
    // Every word survives, in order.
    let rejoined = chunks.join(" ");
    assert_eq!(rejoined, text);
}

/// A word longer than the whole limit cannot be broken politely, but must still
/// be spoken rather than jamming the chunker.
#[test]
fn a_single_word_longer_than_the_limit_is_still_emitted() {
    let policy = ChunkPolicy {
        max_chars: Some(10),
        first_chunk_chars: 0,
    };
    let chunks = chunk_all(policy, &"a".repeat(35));
    assert!(!chunks.is_empty());
    assert_eq!(chunks.concat(), "a".repeat(35));
}

// ---------------------------------------------------------------- boundaries

/// Each of these is audible when it goes wrong: a split here produces two
/// utterances with a pause and a reset intonation in the middle of a phrase.
#[test]
fn abbreviations_do_not_end_a_sentence() {
    for text in [
        "Dr. Smith arrived today.",
        "Fig. 3 shows the result.",
        "Tea, coffee, etc. were served.",
        "Cats, e.g. tabbies, sleep a lot.",
        "Acme Inc. filed the report.",
        "Meet on Mon. at noon.",
    ] {
        let chunks = chunk_all(plain(), text);
        assert_eq!(chunks.len(), 1, "{text:?} must stay one chunk: {chunks:?}");
    }
}

#[test]
fn initials_do_not_end_a_sentence() {
    let chunks = chunk_all(plain(), "J. R. R. Tolkien wrote it.");
    assert_eq!(chunks, vec!["J. R. R. Tolkien wrote it."]);
}

#[test]
fn decimals_versions_and_domains_do_not_end_a_sentence() {
    for text in [
        "Pi is 3.14 exactly.",
        "Upgrade to v1.2 today.",
        "It cost $1,234.56 total.",
        "Visit example.com later.",
    ] {
        let chunks = chunk_all(plain(), text);
        assert_eq!(chunks.len(), 1, "{text:?} split: {chunks:?}");
    }
}

#[test]
fn an_ellipsis_does_not_split_mid_sentence() {
    let chunks = chunk_all(plain(), "Well... it depends on the case.");
    assert_eq!(chunks.len(), 1, "{chunks:?}");
}

#[test]
fn real_boundaries_are_found() {
    let policy = ChunkPolicy {
        max_chars: Some(25),
        first_chunk_chars: 0,
    };
    let chunks = chunk_all(policy, "First one. Second one! Third one? Done.");
    assert!(chunks.len() > 1);
    // Every chunk ends on a terminator, so nothing was cut mid-sentence.
    for chunk in &chunks {
        assert!(
            chunk.ends_with(['.', '!', '?']),
            "chunk cut mid-sentence: {chunk:?}"
        );
    }
}

#[test]
fn a_terminator_inside_quotes_still_ends_the_sentence() {
    let policy = ChunkPolicy {
        max_chars: Some(30),
        first_chunk_chars: 0,
    };
    let chunks = chunk_all(policy, "He said \"go now.\" Then he left the room.");
    assert!(chunks.len() > 1, "{chunks:?}");
    assert!(
        chunks[0].ends_with('"'),
        "the closing quote belongs to the sentence it ends: {:?}",
        chunks[0]
    );
}

#[test]
fn cjk_terminators_end_a_sentence() {
    let policy = ChunkPolicy {
        max_chars: Some(12),
        first_chunk_chars: 0,
    };
    let chunks = chunk_all(policy, "こんにちは。元気ですか。さようなら。");
    assert!(chunks.len() > 1, "{chunks:?}");
    for chunk in &chunks {
        assert!(chunk.ends_with('。'), "{chunk:?}");
    }
}

// ----------------------------------------------------------------- streaming

/// The streaming contract: feeding a delta at a time must produce the same
/// chunks as feeding the whole string. A chunker that split on a terminator
/// before seeing what follows it would fail this on "Dr." arriving alone.
#[test]
fn streaming_a_delta_at_a_time_matches_the_whole_string() {
    let policy = ChunkPolicy {
        max_chars: Some(60),
        first_chunk_chars: 25,
    };
    let cases = [
        "Dr. Smith arrived. He was late, as usual, and everyone noticed it.",
        "Pi is 3.14. Visit example.com for more. Then stop.",
        "One. Two. Three. Four. Five. Six. Seven. Eight.",
    ];
    for case in cases {
        let whole = chunk_all(policy, case);
        let mut c = Chunker::new(policy);
        let mut streamed = Vec::new();
        for ch in case.chars() {
            streamed.extend(c.push(&ch.to_string()));
        }
        streamed.extend(c.finish());
        assert_eq!(streamed, whole, "streaming {case:?} diverged");
    }
}

/// Deterministic xorshift: a failing split has to be reproducible from the
/// seed alone, or the fuzz below is untriageable.
fn xorshift(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

/// `text` cut into deltas of one to nine characters.
fn random_deltas(text: &str, state: &mut u64) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let end = (i + (xorshift(state) % 9) as usize + 1).min(chars.len());
        out.push(chars[i..end].iter().collect());
        i = end;
    }
    out
}

/// The same contract as above, fuzzed: a delta at a time is only one arrival
/// pattern, and the one that broke this was a delta ending *on* the space
/// after a terminator sitting on the chunk target.
#[test]
fn streaming_at_random_delta_sizes_matches_the_whole_string() {
    let policies = [
        ChunkPolicy {
            max_chars: Some(60),
            first_chunk_chars: 25,
        },
        ChunkPolicy {
            max_chars: Some(40),
            first_chunk_chars: 0,
        },
        // What every model with no declared `max_input_chars` gets.
        ChunkPolicy::default(),
    ];
    let cases = [
        "Dr. Smith arrived. He was late, as usual, and everyone noticed it.",
        "Pi is 3.14. Visit example.com for more. Then stop. Then keep going.",
        "One. Two. Three. Four. Five. Six. Seven. Eight. Nine. Ten. Eleven.",
        "He said \"go now.\" Then he left. J. R. R. Tolkien wrote it, vol. 2.",
        "今日は晴れです。明日は雨でしょうか。それはわかりません。もう一度言います。",
        "The kettle is boiling and the cat woke up. Then she left the room \
         without saying anything. A third sentence follows to make it long.",
        "Summary\nEverything passed\nNext steps\nA fourth line, long enough to \
         need a cut somewhere before its end arrives.",
        "Title\nOne. Two. Three.\nitem\nitem two, with a tail. Done.",
    ];
    let mut state = 0x243F_6A88_85A3_08D3;
    for policy in policies {
        for case in cases {
            let whole = chunk_all(policy, case);
            for _ in 0..500 {
                let deltas = random_deltas(case, &mut state);
                let mut c = Chunker::new(policy);
                let mut streamed = Vec::new();
                for delta in &deltas {
                    streamed.extend(c.push(delta));
                }
                streamed.extend(c.finish());
                assert_eq!(
                    streamed, whole,
                    "{policy:?} diverged on {case:?}\n  deltas: {deltas:?}"
                );
            }
        }
    }
}

/// The regression the fuzz found, pinned as its own case.
///
/// The terminator lands exactly on the target, so when a delta ends on the
/// space after it there is nothing yet to tell a sentence end from `etc.`.
/// Judging that as "not a boundary" made the chunker settle for the previous
/// one, and the opener came up a whole sentence short of what the same text
/// pushed whole produces.
#[test]
fn a_boundary_on_the_target_is_not_traded_for_an_earlier_one() {
    let policy = ChunkPolicy {
        max_chars: Some(40),
        first_chunk_chars: 0,
    };
    let case = "One. Two. Three. Four. Five. Six. Seven. Eight. Nine.";
    assert_eq!(case.chars().take(40).collect::<String>().len(), 40);

    let mut c = Chunker::new(policy);
    let mut streamed = Vec::new();
    // "…Six. Seven." ends at char 40; this delta stops on the space after it.
    for delta in ["One. Two. Three. Four. Five. Six. Seven. ", "Eight. Nine."] {
        streamed.extend(c.push(delta));
    }
    streamed.extend(c.finish());
    assert_eq!(
        streamed,
        vec!["One. Two. Three. Four. Five. Six. Seven.", "Eight. Nine."],
    );
    assert_eq!(streamed, chunk_all(policy, case));
}

/// The same edge one step worse: with a model limit in play, the fallback past
/// an unjudgeable boundary is a *word* break, so the chunk used to end
/// mid-sentence — "…ffff gggg" / "hhhh. Next…" — where the one-shot path
/// splits cleanly. A seam fade through the middle of a word is audible.
#[test]
fn a_boundary_on_the_limit_is_not_traded_for_a_word_break() {
    let policy = ChunkPolicy {
        max_chars: Some(40),
        first_chunk_chars: 0,
    };
    // The first sentence is exactly 40 characters, and nothing ends before it.
    let case = "Aaaa bbbb cccc dddd eeee ffff gggg hhhh. Next sentence here.";
    assert_eq!(case.chars().position(|c| c == '.'), Some(39));

    let mut c = Chunker::new(policy);
    let mut streamed = Vec::new();
    for delta in [
        "Aaaa bbbb cccc dddd eeee ffff gggg hhhh. ",
        "Next sentence here.",
    ] {
        streamed.extend(c.push(delta));
    }
    streamed.extend(c.finish());
    assert_eq!(
        streamed,
        vec![
            "Aaaa bbbb cccc dddd eeee ffff gggg hhhh.",
            "Next sentence here."
        ],
    );
    assert_eq!(streamed, chunk_all(policy, case));
}

/// Whatever the split, the words must come out complete and in order — the
/// property that actually matters to a listener.
#[test]
fn chunking_never_loses_or_reorders_words() {
    let policy = ChunkPolicy {
        max_chars: Some(35),
        first_chunk_chars: 15,
    };
    let text = "Dr. Smith arrived at 3.14 pm. He visited example.com first! \
                Then, after a while, he left. Finally it ended.";
    let chunks = chunk_all(policy, text);
    let words: Vec<&str> = text.split_whitespace().collect();
    let got: Vec<String> = chunks
        .iter()
        .flat_map(|c| c.split_whitespace().map(str::to_owned))
        .collect();
    assert_eq!(got, words, "chunks: {chunks:?}");
}

/// A chunker is reused across utterances, so `finish` has to leave it clean —
/// including the "first chunk is short" counter.
#[test]
fn finish_resets_state_for_the_next_utterance() {
    let policy = ChunkPolicy {
        max_chars: None,
        first_chunk_chars: 20,
    };
    let mut c = Chunker::new(policy);
    let first = {
        let mut v = c.push("Short one here. And then some more text after it.");
        v.extend(c.finish());
        v
    };
    let second = {
        let mut v = c.push("Short one here. And then some more text after it.");
        v.extend(c.finish());
        v
    };
    assert_eq!(first, second, "the second utterance must chunk identically");
}

/// A line break is cut where it stands, target or no target: a heading, a
/// list item and a paragraph are separate utterances however short, because a
/// backend never sees the break and would read them as one sentence.
#[test]
fn a_line_break_is_cut_without_waiting_for_the_target() {
    let lines = "Summary\nEverything passed\nNext steps";
    let expected = vec!["Summary", "Everything passed", "Next steps"];
    assert_eq!(chunk_all(plain(), lines), expected);
    assert_eq!(chunk_all(ChunkPolicy::default(), lines), expected);
    // Sentences within a line still pack together, as they always did.
    assert_eq!(
        chunk_all(ChunkPolicy::default(), "Title\nOne. Two. Three."),
        vec!["Title", "One. Two. Three."]
    );
    // A break at the head of the buffer is not an empty chunk.
    let mut c = Chunker::new(plain());
    let mut out = c.push("Title");
    out.extend(c.push("\nBody"));
    out.extend(c.finish());
    assert_eq!(out, vec!["Title", "Body"]);
}

/// A line longer than the model's limit is cut at the limit like any other
/// oversized text, and the break after it is still honored.
#[test]
fn a_line_longer_than_the_limit_is_cut_at_the_limit_then_at_the_break() {
    let policy = ChunkPolicy {
        max_chars: Some(20),
        first_chunk_chars: 0,
    };
    let text = "aaaa bbbb cccc dddd eeee ffff\nnext line";
    let chunks = chunk_all(policy, text);
    assert!(chunks.iter().all(|c| c.chars().count() <= 20), "{chunks:?}");
    assert!(chunks.iter().all(|c| !c.contains('\n')), "{chunks:?}");
    assert_eq!(chunks.last().map(String::as_str), Some("next line"));
    let words: Vec<&str> = chunks.iter().flat_map(|c| c.split_whitespace()).collect();
    assert_eq!(words, text.split_whitespace().collect::<Vec<_>>());
}

/// The whole pipeline, fuzzed: markdown through the normalizer and the chunker
/// a few characters at a time must chunk exactly as the whole string does. The
/// normalizer's line-break verdict needs the start of the next line, so a
/// verdict made one delta early would show here as a different split.
#[test]
fn the_pipeline_chunks_the_same_however_the_deltas_fall() {
    use super::super::normalize::{Normalizer, normalize};
    let policy = ChunkPolicy::default();
    let cases = [
        "## Summary\nEverything passed\n\n- alpha\n- beta\n1. one\n2) two\nThe quick \
         brown fox jumps over\nthe lazy dog. Then, apples,\nAnd plums. *emphasis* \
         opens\n*this* line.",
        "Here:\n```rust\nfn main() {}\n```\nDone\n> quoted\n状態\n正常\nSee \
         [docs](https://x.com/y) now.\r\nWindows line.",
    ];
    let mut state = 0x9E37_79B9_7F4A_7C15;
    for case in cases {
        let whole = chunk_all(policy, &normalize(case));
        assert!(
            whole.len() >= 5,
            "the case should have several lines: {whole:?}"
        );
        for _ in 0..300 {
            let deltas = random_deltas(case, &mut state);
            let mut n = Normalizer::new();
            let mut c = Chunker::new(policy);
            let mut streamed = Vec::new();
            for delta in &deltas {
                streamed.extend(c.push(&n.push(delta)));
            }
            streamed.extend(c.push(&n.finish()));
            streamed.extend(c.finish());
            assert_eq!(
                streamed, whole,
                "diverged on {case:?}\n  deltas: {deltas:?}"
            );
        }
    }
}

/// The report that prompted the line-break rule: a list under a colon, spoken
/// as one sentence. Every item is an utterance of its own, dashes, slashes and
/// parentheses notwithstanding.
#[test]
fn a_bulleted_list_is_one_utterance_per_item() {
    use super::super::normalize::normalize;
    let text = "Practical picks that almost everyone gets value from:\n\
        - Phone stands / tablet holders — takes minutes, used daily\n\
        - Cable clips and organizers — cheap, endlessly useful\n\
        - Broken appliance parts — knobs, brackets, clips, the exact thing you can't buy \
        separately (this is 3D printing's killer app)\n\
        - Batteries / screwdriver organizers — drawers finally make sense\n\
        - Headphone hooks, key holders — mount under a desk or shelf\n\
        - Caliper or tool holders — wall-mounted, satisfying to organize with";
    let chunks = chunk_all(ChunkPolicy::default(), &normalize(text));
    assert_eq!(chunks.len(), 7, "{chunks:?}");
    assert_eq!(
        chunks[0],
        "Practical picks that almost everyone gets value from:"
    );
    assert_eq!(
        chunks[1],
        "Phone stands / tablet holders — takes minutes, used daily"
    );
    assert_eq!(
        chunks[6],
        "Caliper or tool holders — wall-mounted, satisfying to organize with"
    );
    assert!(chunks.iter().all(|c| !c.starts_with('-')), "{chunks:?}");
}
