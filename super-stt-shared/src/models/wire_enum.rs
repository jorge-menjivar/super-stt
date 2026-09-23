// SPDX-License-Identifier: GPL-3.0-only
//! Super STT's settings enums cross the wire and the config file as stable
//! `snake_case` tokens, generated from one table each by
//! `super_engine_protocol::wire_enum_strings!`. These tests hold each enum to
//! its table; the engine's own enums are tested there.

use crate::models::notification_method::NotificationMethod;
use crate::models::protocol::PreviewSource;
use crate::models::recording_stop_mode::RecordingStopMode;
use crate::models::write_method::WriteMethod;

/// Assert the forms this macro generates all agree, for one enum.
///
/// The table is the single source, so the check is that nothing has been
/// hand-written back out of step with it: what `Serialize` emits, what
/// `FromStr` accepts, and — under the `openapi` feature — what the schema
/// publishes as the accepted values.
macro_rules! assert_wire_forms_agree {
    ($ty:ty) => {{
        let variants = <$ty>::WIRE_VARIANTS;
        assert!(!variants.is_empty(), "a wire enum with no variants");

        for wire in variants {
            let parsed: $ty = wire.parse().unwrap_or_else(|e| {
                panic!(
                    "{} does not accept its own token {wire:?}: {e}",
                    stringify!($ty)
                )
            });

            // Serialize must produce the token FromStr took, or a value
            // round-tripped through the daemon comes back as a different
            // variant — or as an error on the far side.
            let json = serde_json::to_string(&parsed).expect("serializes");
            assert_eq!(
                json,
                format!("\"{wire}\""),
                "{} serializes {wire:?} as {json}",
                stringify!($ty),
            );

            // Display is the config-file form and must not drift from the
            // wire form either.
            assert_eq!(
                parsed.to_string(),
                *wire,
                "{} displays {wire:?} differently",
                stringify!($ty),
            );

            let back: $ty = serde_json::from_str(&json).expect("deserializes");
            assert_eq!(
                back.as_wire_str(),
                *wire,
                "{} does not round-trip {wire:?}",
                stringify!($ty),
            );
        }
    }};
}

#[test]
fn every_wire_enum_agrees_with_its_table() {
    assert_wire_forms_agree!(RecordingStopMode);
    assert_wire_forms_agree!(WriteMethod);
    assert_wire_forms_agree!(NotificationMethod);
    assert_wire_forms_agree!(PreviewSource);
}

/// An unrecognized token is refused rather than silently defaulted, so a
/// REST endpoint answers `400` instead of quietly storing something else.
#[test]
fn an_unknown_token_is_refused() {
    let wire = RecordingStopMode::WIRE_VARIANTS[0];
    assert!(
        wire.to_uppercase().parse::<RecordingStopMode>().is_err(),
        "tokens are case-sensitive"
    );
    assert!("".parse::<RecordingStopMode>().is_err());
    assert!(
        format!("{wire} ").parse::<RecordingStopMode>().is_err(),
        "no trimming"
    );
}

/// The published schema must offer exactly the tokens the type accepts.
///
/// This is the reason the macro generates the schema instead of the type
/// deriving `ToSchema`: a derive reads the *Rust* variant names, so it would
/// publish `SciFi` as an accepted value of a field that only ever accepts
/// `scifi`. Nothing type-checks a schema against a hand-written `Serialize`,
/// so the disagreement would ship silently — a generated client would send
/// a value the daemon rejects.
#[cfg(feature = "openapi")]
#[test]
fn every_wire_enum_publishes_exactly_its_tokens() {
    fn published<T: utoipa::PartialSchema>() -> Vec<String> {
        let schema = serde_json::to_value(T::schema()).expect("the schema serializes");
        assert_eq!(
            schema["type"], "string",
            "a wire enum is a string on the wire"
        );
        schema["enum"]
            .as_array()
            .expect("the schema lists its accepted values")
            .iter()
            .map(|v| v.as_str().expect("a token is a string").to_string())
            .collect()
    }

    macro_rules! assert_schema_matches_table {
        ($ty:ty) => {
            assert_eq!(
                published::<$ty>(),
                <$ty>::WIRE_VARIANTS
                    .iter()
                    .map(|s| (*s).to_string())
                    .collect::<Vec<_>>(),
                "{} publishes values it does not accept",
                stringify!($ty),
            );
        };
    }

    assert_schema_matches_table!(RecordingStopMode);
    assert_schema_matches_table!(WriteMethod);
    assert_schema_matches_table!(NotificationMethod);
    assert_schema_matches_table!(PreviewSource);
}
