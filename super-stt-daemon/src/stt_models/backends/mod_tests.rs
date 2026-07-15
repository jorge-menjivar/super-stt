// SPDX-License-Identifier: GPL-3.0-only
use super::*;
use std::fs;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir()
        .join("super-stt-backend-discovery")
        .join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// A WASM backend (OpenAI-shaped) and a subprocess backend (Voxtral-shaped)
/// are both discovered, and their models resolve by `(name, provider, source)`.
#[test]
fn discovers_wasm_and_subprocess_backends() {
    let root = scratch("mixed");

    let openai = root.join("openai");
    fs::create_dir_all(&openai).unwrap();
    fs::write(
        openai.join("backend.toml"),
        r#"
[backend]
source = "github.com/super-stt/openai"
name = "OpenAI"
version = "0.1.0"
kind = "wasm"
entrypoint = "openai.wasm"
contract = "v1"
description = "Test backend."

[network]
allowed_hosts = ["api.openai.com"]

[[secrets]]
name = "OPENAI_API_KEY"
description = "OpenAI API key."
required = true

[[options]]
name = "base_url"
description = "Base URL."
type = "string"
default = "https://api.openai.com"

[[models]]
name = "whisper-1"
provider = "openai"
multilingual = true
primary_language = "en"
supported_languages = ["en"]
supported_devices = ["none"]
"#,
    )
    .unwrap();

    let voxtral = root.join("voxtral");
    fs::create_dir_all(&voxtral).unwrap();
    fs::write(
        voxtral.join("backend.toml"),
        r#"
[backend]
source = "github.com/super-stt/voxtral"
name = "Voxtral (local)"
version = "0.1.0"
kind = "subprocess"
entrypoint = "super-stt-backend-voxtral"
contract = "v1"
description = "Test backend."

[[models]]
name = "voxtral-mini"
provider = "local_voxtral"
multilingual = true
primary_language = "en"
supported_languages = ["en"]
supported_devices = ["cpu", "cuda"]
estimated_vram_bytes = 8589934592
processing_interval_ms = 2000
"#,
    )
    .unwrap();

    let backends = discover(&root);
    assert_eq!(backends.len(), 2, "expected two backends, got {backends:?}");

    // Identity + secrets/options carried through.
    let oai = backends
        .iter()
        .find(|b| b.source == "github.com/super-stt/openai")
        .expect("openai backend");
    assert_eq!(oai.kind, "wasm");
    assert_eq!(oai.entrypoint, "openai.wasm");
    assert_eq!(oai.allowed_hosts, vec!["api.openai.com".to_string()]);
    assert_eq!(oai.secrets.len(), 1);
    assert!(oai.secrets[0].required);
    assert_eq!(oai.options.len(), 1);

    // find_model resolves the triple; provider parses to the newtype.
    let (b, def) = find_model(
        &backends,
        "whisper-1",
        &Provider::from("openai"),
        "github.com/super-stt/openai",
    )
    .expect("resolve whisper-1");
    assert_eq!(b.kind, "wasm");
    assert_eq!(def.source, "github.com/super-stt/openai");
    assert_eq!(
        def.supported_devices,
        vec![super_stt_registry_types::manifest::Device::None],
        "online model carries its declared supported_devices"
    );

    // Empty source matches the first backend serving (name, provider).
    let (_, vox) = find_model(
        &backends,
        "voxtral-mini",
        &Provider::from("local_voxtral"),
        "",
    )
    .expect("resolve voxtral-mini with empty source");
    assert_eq!(vox.source, "github.com/super-stt/voxtral");
    assert_eq!(vox.estimated_vram_bytes, 8_589_934_592);
    assert_eq!(vox.processing_interval, Duration::from_millis(2000));
    assert_eq!(
        vox.supported_devices,
        vec![
            super_stt_registry_types::manifest::Device::Cpu,
            super_stt_registry_types::manifest::Device::Cuda
        ],
        "local model carries its declared supported_devices"
    );

    // list_models flattens both.
    let listed = list_models(&backends);
    assert_eq!(listed.len(), 2);
    assert!(
        listed
            .iter()
            .any(|(n, _, s)| n == "whisper-1" && s == "github.com/super-stt/openai")
    );
}

/// A subdirectory without a parseable `backend.toml` is skipped, not fatal.
#[test]
fn skips_invalid_backend_dirs() {
    let root = scratch("invalid");
    let junk = root.join("not-a-backend");
    fs::create_dir_all(&junk).unwrap();
    fs::write(junk.join("readme.txt"), "hi").unwrap();
    let broken = root.join("broken");
    fs::create_dir_all(&broken).unwrap();
    fs::write(broken.join("backend.toml"), "this is not valid toml = =\n").unwrap();

    assert!(discover(&root).is_empty());
}

#[test]
fn missing_dir_is_empty() {
    let path = std::env::temp_dir().join("super-stt-backend-discovery/does-not-exist");
    assert!(discover(&path).is_empty());
}

/// A backend whose manifest omits `supported_devices` on any model is
/// rejected at discovery — the field is required.
#[test]
fn missing_supported_devices_skips_backend() {
    let root = scratch("missing_devices");
    let openai = root.join("openai");
    fs::create_dir_all(&openai).unwrap();
    fs::write(
        openai.join("backend.toml"),
        r#"
[backend]
source = "github.com/super-stt/openai"
name = "OpenAI"
version = "0.1.0"
kind = "wasm"
entrypoint = "openai.wasm"
contract = "v1"
description = "Test backend."

[[models]]
name = "whisper-1"
provider = "openai"
multilingual = true
# supported_devices intentionally absent
"#,
    )
    .unwrap();

    assert!(
        discover(&root).is_empty(),
        "a manifest without supported_devices must be rejected"
    );
}

/// A backend whose manifest has an explicit empty `supported_devices = []` on a
/// model is rejected at discovery — the empty-list bail in
/// `validate_supported_devices` must be reached even when the field is present.
#[test]
fn empty_supported_devices_skips_backend() {
    let root = scratch("empty_devices");
    let openai = root.join("openai");
    fs::create_dir_all(&openai).unwrap();
    fs::write(
        openai.join("backend.toml"),
        r#"
[backend]
source = "github.com/super-stt/openai"
name = "OpenAI"
version = "0.1.0"
kind = "wasm"
entrypoint = "openai.wasm"
contract = "v1"
description = "Test backend."

[[models]]
name = "whisper-1"
provider = "openai"
multilingual = true
primary_language = "en"
supported_languages = ["en"]
supported_devices = []
"#,
    )
    .unwrap();

    assert!(
        discover(&root).is_empty(),
        "a manifest with supported_devices = [] must be rejected"
    );
}

/// Unknown device strings (`xpu`) cause the whole backend to be skipped.
#[test]
fn unknown_device_skips_backend() {
    let root = scratch("unknown_device");
    let openai = root.join("openai");
    fs::create_dir_all(&openai).unwrap();
    fs::write(
        openai.join("backend.toml"),
        r#"
[backend]
source = "github.com/super-stt/openai"
name = "OpenAI"
version = "0.1.0"
kind = "wasm"
entrypoint = "openai.wasm"
contract = "v1"
description = "Test backend."

[[models]]
name = "whisper-1"
provider = "openai"
multilingual = true
primary_language = "en"
supported_languages = ["en"]
supported_devices = ["xpu"]
"#,
    )
    .unwrap();

    assert!(discover(&root).is_empty());
}

/// `none` (online sentinel) mixed with a local device is rejected — they
/// contradict each other.
#[test]
fn none_mixed_with_local_device_skips_backend() {
    let root = scratch("mixed_none");
    let openai = root.join("openai");
    fs::create_dir_all(&openai).unwrap();
    fs::write(
        openai.join("backend.toml"),
        r#"
[backend]
source = "github.com/super-stt/openai"
name = "OpenAI"
version = "0.1.0"
kind = "wasm"
entrypoint = "openai.wasm"
contract = "v1"
description = "Test backend."

[[models]]
name = "whisper-1"
provider = "openai"
multilingual = true
primary_language = "en"
supported_languages = ["en"]
supported_devices = ["none", "cpu"]
"#,
    )
    .unwrap();

    assert!(discover(&root).is_empty());
}

/// `dir_name` returns the final path component — the relative install dir
/// the daemon persists in `config.transcription.active_backend`. A trailing
/// slash and a root-only path both return `None` (no usable handle).
#[test]
fn dir_name_returns_final_component() {
    use crate::stt_models::ModelDefinition;

    fn fake_backend(dir: PathBuf) -> DiscoveredBackend {
        DiscoveredBackend {
            dir,
            source: "github.com/super-stt/openai".to_string(),
            name: "OpenAI".to_string(),
            kind: "wasm".to_string(),
            entrypoint: "openai.wasm".to_string(),
            allowed_hosts: Vec::new(),
            secrets: Vec::new(),
            options: Vec::new(),
            models: Vec::<ModelDefinition>::new(),
        }
    }

    let nested = fake_backend(PathBuf::from(
        "/home/u/.local/share/super-stt/backends/openai",
    ));
    assert_eq!(dir_name(&nested).as_deref(), Some("openai"));

    let with_trailing_slash = fake_backend(PathBuf::from(
        "/home/u/.local/share/super-stt/backends/mistral/",
    ));
    assert_eq!(dir_name(&with_trailing_slash).as_deref(), Some("mistral"));

    let root_only = fake_backend(PathBuf::from("/"));
    assert!(
        dir_name(&root_only).is_none(),
        "a root path has no file_name → None"
    );
}

/// Write a minimal single-model wasm backend into `root/<dir>` with the
/// given `source`. Enough for discovery to succeed.
fn write_backend(root: &Path, dir: &str, source: &str, name: &str) {
    let d = root.join(dir);
    fs::create_dir_all(&d).unwrap();
    fs::write(
        d.join("backend.toml"),
        format!(
            r#"
[backend]
source = "{source}"
name = "{name}"
version = "0.1.0"
kind = "wasm"
entrypoint = "{dir}.wasm"
contract = "v1"
description = "Test backend."

[[models]]
name = "{dir}-base"
provider = "openai"
multilingual = true
primary_language = "en"
supported_languages = ["en"]
supported_devices = ["none"]
"#
        ),
    )
    .unwrap();
}

/// The three monorepo backends each declare a distinct source namespaced
/// under the shared repo, so resolving the active backend by source —
/// exactly what `handle_set_active_backend` does
/// (`find(|b| b.source == source).and_then(dir_name)`) — lands on the
/// requested backend. This is the regression test for the bug where all
/// three shared one source and selecting Voxtral activated Mistral.
#[test]
fn distinct_sources_resolve_to_the_right_backend() {
    let root = scratch("distinct-sources");
    write_backend(
        &root,
        "openai",
        "github.com/jorge-menjivar/super-stt/openai",
        "OpenAI",
    );
    write_backend(
        &root,
        "mistral",
        "github.com/jorge-menjivar/super-stt/mistral",
        "Mistral",
    );
    write_backend(
        &root,
        "voxtral",
        "github.com/jorge-menjivar/super-stt/voxtral",
        "Voxtral",
    );

    let backends = discover(&root);
    assert_eq!(backends.len(), 3);

    for (source, want_dir) in [
        ("github.com/jorge-menjivar/super-stt/openai", "openai"),
        ("github.com/jorge-menjivar/super-stt/mistral", "mistral"),
        ("github.com/jorge-menjivar/super-stt/voxtral", "voxtral"),
    ] {
        let resolved = backends
            .iter()
            .find(|b| b.source == source)
            .and_then(dir_name);
        assert_eq!(
            resolved.as_deref(),
            Some(want_dir),
            "source {source} should resolve to dir {want_dir}"
        );
    }
}

/// Two backends sharing a source is a misconfiguration: discovery keeps the
/// first and drops the rest so resolution is never ambiguous.
#[test]
fn duplicate_sources_are_deduplicated() {
    let root = scratch("dup-sources");
    // Same source on two different dirs — the pre-fix bug condition.
    write_backend(&root, "aaa", "github.com/x/shared", "First");
    write_backend(&root, "bbb", "github.com/x/shared", "Second");

    let backends = discover(&root);
    assert_eq!(
        backends.len(),
        1,
        "duplicate source must be collapsed to one"
    );

    // Exactly one backend resolves for the shared source — no ambiguity.
    let matches: Vec<_> = backends
        .iter()
        .filter(|b| b.source == "github.com/x/shared")
        .collect();
    assert_eq!(matches.len(), 1);
}

/// The qwen3-asr subprocess backend is discovered and both of its models
/// resolve with the new `local_qwen3_asr` provider.
#[test]
fn discovers_qwen3_asr_backend() {
    let root = scratch("qwen3");
    let dir = root.join("qwen3-asr");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("backend.toml"),
        r#"
[backend]
source = "github.com/jorge-menjivar/super-stt/qwen3-asr"
name = "Qwen3-ASR"
version = "0.1.0"
kind = "subprocess"
entrypoint = "qwen3-asr"
contract = "v1"
description = "Test backend."

[[models]]
name = "qwen3-asr-0.6b"
provider = "local_qwen3_asr"
multilingual = true
primary_language = "en"
supported_languages = ["en"]
supported_devices = ["cpu", "cuda"]
estimated_vram_bytes = 2500000000
processing_interval_ms = 1000

[[models]]
name = "qwen3-asr-1.7b"
provider = "local_qwen3_asr"
multilingual = true
primary_language = "en"
supported_languages = ["en"]
supported_devices = ["cpu", "cuda"]
estimated_vram_bytes = 6000000000
processing_interval_ms = 1500
"#,
    )
    .unwrap();

    let backends = discover(&root);
    assert_eq!(backends.len(), 1);

    let (b, def) = find_model(
        &backends,
        "qwen3-asr-0.6b",
        &Provider::from("local_qwen3_asr"),
        "github.com/jorge-menjivar/super-stt/qwen3-asr",
    )
    .expect("resolve qwen3-asr-0.6b");
    assert_eq!(b.kind, "subprocess");
    assert_eq!(b.entrypoint, "qwen3-asr");
    assert_eq!(
        def.supported_devices,
        vec![
            super_stt_registry_types::manifest::Device::Cpu,
            super_stt_registry_types::manifest::Device::Cuda
        ]
    );

    let (_, big) = find_model(
        &backends,
        "qwen3-asr-1.7b",
        &Provider::from("local_qwen3_asr"),
        "",
    )
    .expect("resolve qwen3-asr-1.7b with empty source");
    assert_eq!(big.estimated_vram_bytes, 6_000_000_000);
    assert_eq!(big.processing_interval, Duration::from_millis(1500));
}

/// `dedup_sources` keeps first-seen order and drops later duplicates.
#[test]
fn dedup_sources_keeps_first_occurrence() {
    use crate::stt_models::ModelDefinition;
    fn fake(dir: &str, source: &str) -> DiscoveredBackend {
        DiscoveredBackend {
            dir: PathBuf::from(dir),
            source: source.to_string(),
            name: dir.to_string(),
            kind: "wasm".to_string(),
            entrypoint: "x.wasm".to_string(),
            allowed_hosts: Vec::new(),
            secrets: Vec::new(),
            options: Vec::new(),
            models: Vec::<ModelDefinition>::new(),
        }
    }
    let input = vec![
        fake("a", "src-1"),
        fake("b", "src-2"),
        fake("c", "src-1"), // dup of a
        fake("d", "src-3"),
    ];
    let out = dedup_sources(input);
    let dirs: Vec<_> = out.iter().filter_map(dir_name).collect();
    assert_eq!(dirs, vec!["a", "b", "d"]);
}
