// SPDX-License-Identifier: GPL-3.0-only
//! Installer and self-updater for Super STT. One binary serves the curl
//! bootstrap (interactive), the app's in-app update (--non-interactive
//! --json-progress), and the escalated file-placement step (--root-phase).

mod cli;
mod download;
mod errors;
mod escalate;
mod post_install;
mod progress;
mod resolve;
mod root_phase;
mod stage;
mod verify;

use std::io::IsTerminal;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};

use errors::InstallError;
use progress::{Event, Phase, Reporter};

/// Owns the installer's private staging directory: mode `0700` and an
/// unpredictable name (pid + random suffix) so a local user can neither
/// pre-create it nor race its creation to plant a symlink, closing the
/// user-writable-staging race the root phase (running with no visibility
/// into how staging came to be) can't itself defend against. Canonicalized
/// immediately after creation so the manifest/staged sources are recorded
/// under a real path — the root phase's containment check canonicalizes
/// `staging_root` but compares entry sources to it lexically, so a
/// non-canonical (but equivalent, e.g. via a symlinked temp-dir ancestor)
/// path here would otherwise fail closed for no real reason. Removed on
/// drop so a cancelled/failed run never leaks the extracted, multi-hundred-
/// MB release tree — the same reason the shell installer used a `trap`.
struct StagingGuard {
    path: PathBuf,
}

impl StagingGuard {
    fn new() -> Result<Self, InstallError> {
        let name = format!(
            "super-stt-install-{}-{}",
            std::process::id(),
            random_suffix()
        );
        let path = std::env::temp_dir().join(name);
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&path)
            .map_err(|e| {
                InstallError::InstallFailed(format!("create staging dir {}: {e}", path.display()))
            })?;
        let path = std::fs::canonicalize(&path).map_err(|e| {
            InstallError::InstallFailed(format!("resolve staging dir {}: {e}", path.display()))
        })?;
        Ok(Self { path })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for StagingGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// 8 random bytes, hex-encoded — the unpredictable half of the staging
/// directory's name (obligation (b): pid alone is guessable/reused).
fn random_suffix() -> String {
    use ring::rand::{SecureRandom, SystemRandom};
    let mut buf = [0u8; 8];
    // A failing system RNG means something is deeply wrong with the host;
    // there's no sane "unpredictable" fallback, so this is fatal.
    SystemRandom::new()
        .fill(&mut buf)
        .expect("system RNG unavailable");
    hex::encode(buf)
}

/// The interactive component-selection menu (`scripts/install-beta.sh:397-460`),
/// printed to stderr and read from `/dev/tty` (independent of stdin, which
/// `--json-progress` callers may be piping/redirecting). Returns `None` on
/// `q`/EOF (the caller exits 0, mirroring the script's "cancelled").
fn run_interactive_menu(
    triple: &str,
    cosmic_available: bool,
) -> Result<Option<stage::ComponentSelection>, InstallError> {
    use std::io::{BufRead, Write};
    let tty = std::fs::File::open("/dev/tty")
        .map_err(|e| InstallError::InstallFailed(format!("open /dev/tty: {e}")))?;
    let mut reader = std::io::BufReader::new(tty);
    loop {
        eprintln!("=============================================");
        eprintln!("      Super STT Installation Menu");
        eprintln!("=============================================");
        eprintln!();
        eprintln!("Detected system:");
        eprintln!("  Architecture: {triple}");
        eprintln!();
        eprintln!("What would you like to install?");
        eprintln!();
        eprintln!(
            "1. All {}",
            if cosmic_available {
                "(includes COSMIC applet)"
            } else {
                "(skip COSMIC applet)"
            }
        );
        eprintln!("2. Daemon + CLI only");
        eprintln!("3. Desktop App only");
        eprintln!(
            "4. COSMIC Applet only{}",
            if cosmic_available {
                ""
            } else {
                " (not available)"
            }
        );
        eprintln!();
        eprintln!("q. Quit");
        eprintln!();
        eprintln!("=============================================");
        eprint!("Select option [1-4, q] (default: 1): ");
        let _ = std::io::stderr().flush();

        let mut line = String::new();
        let n = reader
            .read_line(&mut line)
            .map_err(|e| InstallError::InstallFailed(format!("read /dev/tty: {e}")))?;
        if n == 0 {
            // EOF on /dev/tty — treat like an explicit quit.
            return Ok(None);
        }
        match line.trim() {
            "" | "1" => return Ok(Some(stage::ComponentSelection::All)),
            "2" => return Ok(Some(stage::ComponentSelection::Daemon)),
            "3" => return Ok(Some(stage::ComponentSelection::App)),
            "4" if cosmic_available => return Ok(Some(stage::ComponentSelection::Applet)),
            "4" => eprintln!("COSMIC panel not found - applet not available"),
            "q" | "Q" => return Ok(None),
            _ => eprintln!("Invalid option. Please try again."),
        }
    }
}

async fn run(cli: &cli::Cli, reporter: Reporter) -> Result<(), InstallError> {
    reporter.emit(&Event::Phase {
        phase: Phase::Resolve,
        message: "resolving release",
    });
    let triple = resolve::target_triple()?;
    let repo = super_stt_forge::RepoRef::parse(resolve::REPO)
        .expect("REPO is a valid host/owner/repo reference");
    let client = super_stt_forge::Github::from_env();
    let target =
        resolve::resolve_target(&client, &repo, cli.version.as_deref(), cli.beta, triple).await?;

    let path_env = std::env::var("PATH").unwrap_or_default();
    let cosmic_available = escalate::which("cosmic-panel", &path_env).is_some();

    let mut explicit = cli.components;
    if explicit.is_none() && !cli.non_interactive && std::io::stderr().is_terminal() {
        match run_interactive_menu(triple, cosmic_available)? {
            Some(selection) => explicit = Some(selection),
            None => return Ok(()),
        }
    }

    // Staging dir: 0700, unpredictable name, canonicalized — see
    // `StagingGuard`'s doc comment (obligation (b)). RAII-cleaned on every
    // exit path, including an early `?` return.
    let staging = StagingGuard::new()?;

    reporter.emit(&Event::Phase {
        phase: Phase::Download,
        message: &format!("downloading {}", target.tarball_name),
    });
    let tarball_path = staging.path().join(&target.tarball_name);
    download::download_to_file(
        &target.tarball_url,
        &tarball_path,
        |bytes_done, bytes_total| {
            reporter.emit(&Event::Progress {
                phase: Phase::Download,
                bytes_done,
                bytes_total,
            });
        },
    )
    .await?;
    let sums = download::download_string(&target.sums_url, 64 * 1024).await?;

    reporter.emit(&Event::Phase {
        phase: Phase::Verify,
        message: "verifying checksum",
    });
    verify::verify_file(&tarball_path, &target.tarball_name, &sums)?;

    reporter.emit(&Event::Phase {
        phase: Phase::Stage,
        message: "staging files",
    });
    let extracted = staging.path().join("extracted");
    stage::extract_tarball(&tarball_path, &extracted)?;

    let prefix = Path::new("/usr/local");
    let unit_dir = Path::new("/usr/lib/systemd/user");
    let components = stage::plan_components(explicit, prefix, cosmic_available);
    // Captured before the root phase runs: whether the applet was already
    // installed decides whether the panel needs restarting to pick up a
    // *changed* binary, not whether it's present after this run.
    let applet_was_installed = prefix.join("bin/super-stt-cosmic-applet").exists();
    let self_exe = std::env::current_exe()
        .map_err(|e| InstallError::InstallFailed(format!("current_exe: {e}")))?;
    let manifest = stage::build_manifest(&extracted, prefix, unit_dir, &components, &self_exe)?;
    let manifest_path = staging.path().join("manifest.json");
    let manifest_json = serde_json::to_string(&manifest)
        .map_err(|e| InstallError::InstallFailed(format!("serialize manifest: {e}")))?;
    std::fs::write(&manifest_path, manifest_json).map_err(|e| {
        InstallError::InstallFailed(format!("write {}: {e}", manifest_path.display()))
    })?;

    if cli.dry_run {
        reporter.emit(&Event::Complete {
            installed_version: &target.release.tag,
            components: &components.names(),
        });
        return Ok(());
    }

    reporter.emit(&Event::Phase {
        phase: Phase::Escalate,
        message: "waiting for authorization",
    });
    let stderr_tty = std::io::stderr().is_terminal();
    let has_sudo = escalate::which("sudo", &path_env).is_some();
    let has_pkexec = escalate::which("pkexec", &path_env).is_some();
    let method = escalate::pick_method(stderr_tty, has_sudo, has_pkexec)?;
    reporter.emit(&Event::Phase {
        phase: Phase::Install,
        message: "installing files",
    });
    escalate::run_root_phase(method, &manifest_path).await?;

    reporter.emit(&Event::Phase {
        phase: Phase::PostInstall,
        message: "finishing installation",
    });
    let interactive = !cli.non_interactive && std::io::stderr().is_terminal();
    post_install::run(&components, applet_was_installed, interactive, prefix).await?;

    reporter.emit(&Event::Complete {
        installed_version: &target.release.tag,
        components: &components.names(),
    });
    Ok(())
}

fn main() -> std::process::ExitCode {
    super_stt_forge::install_crypto_provider();
    env_logger::Builder::from_default_env()
        .filter_level(log::LevelFilter::Info)
        .init();
    let cli = cli::parse_cli();

    // Escalated re-exec: sync, no runtime, no env dependence.
    if let Some(manifest) = cli.root_phase {
        return std::process::ExitCode::from(root_phase::run(&manifest));
    }

    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    let reporter = if cli.json_progress {
        Reporter::Json
    } else {
        Reporter::Human
    };
    match runtime.block_on(run(&cli, reporter)) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            reporter.emit(&Event::Error {
                code: e.code(),
                message: &e.to_string(),
            });
            std::process::ExitCode::FAILURE
        }
    }
}
