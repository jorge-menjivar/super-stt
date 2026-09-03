// SPDX-License-Identifier: GPL-3.0-only
//! One table → one wire form for fieldless settings enums.
//!
//! `RecordingStopMode`, `WriteMethod`, and `AudioTheme` each cross the wire and
//! the config file as a stable `snake_case` token. Previously each carried three
//! hand-maintained forms (`PascalCase` serde, kebab `Display`, aliased `FromStr`)
//! that had drifted. [`wire_enum_strings!`] generates `Display`, `FromStr`,
//! serde `Serialize`/`Deserialize`, and (under the `openapi` feature) the
//! `utoipa` schema from a single table so they can't.
//!
//! **Unknown-value policy.** `FromStr` and `Deserialize` reject an unrecognized
//! token (so REST endpoints answer `400`, not a silent default). Config-load
//! resilience — an unknown stored value degrading to the field default instead
//! of failing the whole parse — is a separate concern handled by
//! `deserialize_or_default` on the individual config field. There are no legacy
//! aliases: exactly one token maps to each variant.

/// Generate the wire-string plumbing (`as_wire_str`, `Display`, `FromStr`,
/// serde `Serialize`/`Deserialize`) for a fieldless enum from a single table of
/// `Variant => "snake_case_token"` pairs. The enum keeps its own derives
/// (`Debug`, `Clone`, `Copy`, `Default`, …) and any domain methods; it must
/// **not** also derive `Serialize`/`Deserialize` (this macro provides them).
macro_rules! wire_enum_strings {
    ($ty:ident { $($variant:ident => $wire:literal),+ $(,)? }) => {
        impl $ty {
            /// The stable `snake_case` wire/config token for this variant.
            #[must_use]
            pub fn as_wire_str(self) -> &'static str {
                match self { $( Self::$variant => $wire, )+ }
            }

            /// Every accepted wire/config token, in declaration order. Lets a
            /// CLI build its `value_parser` (or help text) from the single
            /// table instead of re-listing the strings.
            pub const WIRE_VARIANTS: &'static [&'static str] = &[ $( $wire, )+ ];
        }

        impl ::std::fmt::Display for $ty {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                f.write_str(self.as_wire_str())
            }
        }

        impl ::std::str::FromStr for $ty {
            type Err = String;
            fn from_str(s: &str) -> ::std::result::Result<Self, Self::Err> {
                match s {
                    $( $wire => ::std::result::Result::Ok(Self::$variant), )+
                    other => ::std::result::Result::Err(
                        ::std::format!(concat!("unknown ", stringify!($ty), ": {}"), other)
                    ),
                }
            }
        }

        impl ::serde::Serialize for $ty {
            fn serialize<S: ::serde::Serializer>(&self, s: S) -> ::std::result::Result<S::Ok, S::Error> {
                s.serialize_str(self.as_wire_str())
            }
        }

        impl<'de> ::serde::Deserialize<'de> for $ty {
            fn deserialize<D: ::serde::Deserializer<'de>>(d: D) -> ::std::result::Result<Self, D::Error> {
                let s = <::std::string::String as ::serde::Deserialize>::deserialize(d)?;
                s.parse().map_err(::serde::de::Error::custom)
            }
        }

        // The OpenAPI schema comes off the same table as `Serialize` and
        // `FromStr`, for the same reason those do. `#[derive(ToSchema)]` reads
        // the *Rust* variant names, so it would publish `SciFi` as an accepted
        // value of a field that only ever accepts `sci_fi` — a spec that
        // disagrees with the endpoint it documents, and silently, since nothing
        // type-checks a schema against a hand-written `Serialize`.
        #[cfg(feature = "openapi")]
        impl ::utoipa::PartialSchema for $ty {
            fn schema() -> ::utoipa::openapi::RefOr<::utoipa::openapi::schema::Schema> {
                ::utoipa::openapi::ObjectBuilder::new()
                    .schema_type(::utoipa::openapi::Type::String)
                    .enum_values(::std::option::Option::Some(
                        Self::WIRE_VARIANTS.iter().copied(),
                    ))
                    .into()
            }
        }

        #[cfg(feature = "openapi")]
        impl ::utoipa::ToSchema for $ty {
            fn name() -> ::std::borrow::Cow<'static, str> {
                ::std::borrow::Cow::Borrowed(::std::stringify!($ty))
            }
        }
    };
}

pub(crate) use wire_enum_strings;

#[cfg(test)]
mod tests {
    use crate::models::notification_method::NotificationMethod;
    use crate::models::recording_stop_mode::RecordingStopMode;
    use crate::models::theme::AudioTheme;
    use crate::models::update_beta_optin::UpdateBetaOptIn;
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
        assert_wire_forms_agree!(AudioTheme);
        assert_wire_forms_agree!(RecordingStopMode);
        assert_wire_forms_agree!(WriteMethod);
        assert_wire_forms_agree!(NotificationMethod);
        assert_wire_forms_agree!(UpdateBetaOptIn);
    }

    /// An unrecognized token is refused rather than silently defaulted, so a
    /// REST endpoint answers `400` instead of quietly storing something else.
    #[test]
    fn an_unknown_token_is_refused() {
        assert!(
            "SciFi".parse::<AudioTheme>().is_err(),
            "the Rust name is not a wire token"
        );
        assert!(serde_json::from_str::<AudioTheme>("\"SciFi\"").is_err());
        assert!("".parse::<AudioTheme>().is_err());
        assert!("classic ".parse::<AudioTheme>().is_err(), "no trimming");
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

        assert_schema_matches_table!(AudioTheme);
        assert_schema_matches_table!(RecordingStopMode);
        assert_schema_matches_table!(WriteMethod);
        assert_schema_matches_table!(NotificationMethod);
        assert_schema_matches_table!(UpdateBetaOptIn);
    }
}
