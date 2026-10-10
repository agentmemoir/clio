// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the `docker` CLI invocation boundary of the
//! `clio compose` command: target (OS/arch) detection, the runner abstraction,
//! the daemon probe, and the service-selection to Compose-profile mapping.
//!
//! ## Owns
//! - Supported-target detection (`std::env::consts::{OS, ARCH}` to overlay file)
//! - The `DockerRunner` seam and its real `docker` CLI implementation
//! - The Docker CLI + daemon reachability probe, and `DockerStatus`, the value
//!   the wizard's questions read to decide which choices it may offer
//! - The service allowlist and the explicit service list per stack
//! - The fixed password-rotation SQL and `compose exec` argument shape
//!
//! ## Does not own
//! - Asset materialization and bind-mount checks (see `compose_assets`)
//! - `.env` generation or the selection marker file (see `compose_env`)
//! - The `clio compose` command surface and exit-code mapping (see `compose_cli`)
//!
//! ## Boundary
//!
//! Changes to which Docker arguments are run or which targets are supported
//! should normally require changes here. Changes to file materialization
//! should not require changes here.
//!
//! Keep this module focused on the Docker invocation boundary.

use std::io;
use std::path::Path;

/// The two supported platform targets, each mapping to its checked-in arch
/// overlay file (external contract: these file names).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ComposeTarget {
    MacArm64,
    LinuxAmd64,
}

impl ComposeTarget {
    /// Map the running OS/arch onto a supported target; fail closed otherwise.
    pub(crate) fn detect(os: &str, arch: &str) -> Result<Self, String> {
        match (os, arch) {
            ("macos", "aarch64") => Ok(Self::MacArm64),
            ("linux", "x86_64") => Ok(Self::LinuxAmd64),
            _ => Err(format!(
                "unsupported platform {os}/{arch}; `clio compose` supports macOS arm64 (aarch64) \
                 and Linux x86_64 only"
            )),
        }
    }

    /// The checked-in arch overlay selected for this target.
    pub(crate) fn overlay(self) -> &'static str {
        match self {
            Self::MacArm64 => "compose.arm64.yml",
            Self::LinuxAmd64 => "compose.amd64.yml",
        }
    }
}

/// The Docker command seam. `capture` collects output (for the probe, whose
/// output is never echoed); the inherited-stdio runs attach the child's stdio
/// to the terminal so `docker compose up|down` output streams to the user.
///
/// The env-carrying variant exists so a value Compose has to resolve can reach
/// the child without ever becoming an argv element. The managed database's
/// chosen port travels that way, so the container and the stored address cannot
/// disagree about which port is published.
pub(crate) trait DockerRunner {
    fn capture(&self, args: &[&str], dir: &Path) -> io::Result<(i32, String)>;
    fn run_inherited(&self, args: &[&str], dir: &Path) -> io::Result<i32>;

    /// Run with the child's stdio inherited and `env` set on the child.
    ///
    /// The default is a loud error, never a silent run without the environment:
    /// a runner used for a call that needs a value to reach Docker must override
    /// this.
    fn run_inherited_env(
        &self,
        _args: &[&str],
        _dir: &Path,
        _env: &[(String, String)],
    ) -> io::Result<i32> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "this DockerRunner cannot pass an environment",
        ))
    }
}

/// Real runner that shells out to the `docker` CLI (no Docker SDK dependency).
pub(crate) struct RealDocker;

impl DockerRunner for RealDocker {
    fn capture(&self, args: &[&str], dir: &Path) -> io::Result<(i32, String)> {
        let out = std::process::Command::new("docker")
            .args(args)
            .current_dir(dir)
            .output()?;
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        Ok((out.status.code().unwrap_or(1), text))
    }

    fn run_inherited(&self, args: &[&str], dir: &Path) -> io::Result<i32> {
        Ok(std::process::Command::new("docker")
            .args(args)
            .current_dir(dir)
            .status()?
            .code()
            .unwrap_or(1))
    }

    fn run_inherited_env(
        &self,
        args: &[&str],
        dir: &Path,
        env: &[(String, String)],
    ) -> io::Result<i32> {
        Ok(std::process::Command::new("docker")
            .args(args)
            .envs(
                env.iter()
                    .map(|(key, value)| (key.as_str(), value.as_str())),
            )
            .current_dir(dir)
            .status()?
            .code()
            .unwrap_or(1))
    }
}

/// Whether this machine can run the containers Clio needs.
///
/// Three states exist in the world, not two: Docker is running, Docker is
/// installed but stopped, and Docker is not installed at all. The second and
/// third both mean Clio cannot start anything, and the reason differs enough to
/// be worth telling apart, so both carry their own message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DockerStatus {
    /// The CLI and the daemon both answered.
    Available,
    /// Clio cannot start a container, and this is why.
    Unavailable(String),
}

impl DockerStatus {
    /// Whether the managed choices may be taken.
    #[must_use]
    pub(crate) fn is_available(&self) -> bool {
        matches!(self, Self::Available)
    }

    /// Why the managed choices cannot be taken, or an empty string.
    #[must_use]
    pub(crate) fn reason(&self) -> &str {
        match self {
            Self::Available => "",
            Self::Unavailable(reason) => reason,
        }
    }
}

/// The status of Docker on this machine, as one value.
///
/// The CLI is asked first and the daemon second, so "not installed" and
/// "installed but stopped" produce different advice: one needs an install and
/// the other needs a click. Output is captured and never printed.
pub(crate) fn status(runner: &dyn DockerRunner) -> DockerStatus {
    match probe(runner) {
        Ok(()) => DockerStatus::Available,
        Err(reason) => DockerStatus::Unavailable(reason),
    }
}

/// Verify the `docker` CLI (with the Compose v2 plugin) and the daemon are
/// reachable. Output is captured and never printed.
pub(crate) fn probe(runner: &dyn DockerRunner) -> Result<(), String> {
    let dir = Path::new(".");
    let (code, _) = runner
        .capture(&["compose", "version"], dir)
        .map_err(|e| format!("docker CLI not found: {e}"))?;
    if code != 0 {
        return Err(
            "docker compose plugin not available; install Docker Engine with the Compose v2 plugin"
                .into(),
        );
    }
    let (code, _) = runner
        .capture(&["info", "--format", "{{.ServerVersion}}"], dir)
        .map_err(|e| format!("docker CLI not found: {e}"))?;
    if code != 0 {
        return Err(
            "docker daemon is not reachable; start Docker Desktop / the Docker daemon first".into(),
        );
    }
    Ok(())
}

/// Every Compose service name a run may name explicitly. User input never
/// becomes an argv token: a name is looked up here and the allowlisted constant
/// is the value that gets stored.
const ALLOWED_SERVICES: [&str; 4] = ["postgres", "embed", "rerank", "extract"];
const MAX_SERVICES: usize = ALLOWED_SERVICES.len();

/// A service selection: the exact allowlisted Compose service names a run
/// should name on the command line, plus the profile the checked-in files
/// require for them.
///
/// The names are exact because `postgres` carries no `profiles:` key: it is
/// Compose's default data plane, so `--profile local-retrieval up -d` starts it
/// whatever the profile says. Naming the chosen services explicitly is the
/// half of that fix a profile cannot provide, and a subset such as
/// `postgres,embed` must stay a subset rather than growing `rerank` back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Stack {
    names: [&'static str; MAX_SERVICES],
    len: usize,
}

#[allow(
    non_upper_case_globals,
    reason = "kept as the enum-variant spellings existing callers already use"
)]
impl Stack {
    /// `postgres` alone: the managed database, no model sidecars.
    pub(crate) const PostgresOnly: Stack = Stack::fixed(["postgres"]);
    /// Managed Postgres plus both local retrieval sidecars.
    pub(crate) const LocalRetrieval: Stack = Stack::fixed(["postgres", "embed", "rerank"]);
    /// The retrieval stack plus the local extraction sidecar.
    pub(crate) const LocalRft: Stack = Stack::fixed(["postgres", "embed", "rerank", "extract"]);

    /// Build one fixed-capacity stack from a literal list of allowlisted names.
    const fn fixed<const N: usize>(names: [&'static str; N]) -> Self {
        let mut slots = [""; MAX_SERVICES];
        let mut index = 0;
        while index < N {
            slots[index] = names[index];
            index += 1;
        }
        Self {
            names: slots,
            len: N,
        }
    }

    /// The exact allowlisted services this stack names, in order.
    pub(crate) fn services(&self) -> &[&'static str] {
        &self.names[..self.len]
    }

    /// Map an allowlisted service list onto a stack.
    ///
    /// Only the checked-in service names are accepted; anything else fails
    /// closed. An empty list stays empty — the "start nothing" case — rather
    /// than falling back to Compose's default data plane.
    ///
    /// # Errors
    /// A name outside the allowlist, or more services than exist.
    pub(crate) fn from_services(services: &[&str]) -> Result<Self, String> {
        if services.len() > MAX_SERVICES {
            return Err(format!(
                "at most {MAX_SERVICES} services; allowed services: {}",
                ALLOWED_SERVICES.join(", ")
            ));
        }
        let mut slots = [""; MAX_SERVICES];
        for (slot, name) in slots.iter_mut().zip(services.iter().copied()) {
            *slot = ALLOWED_SERVICES
                .iter()
                .copied()
                .find(|allowed| *allowed == name)
                .ok_or_else(|| {
                    format!(
                        "unknown service `{name}`; allowed services: {}",
                        ALLOWED_SERVICES.join(", ")
                    )
                })?;
        }
        Ok(Self {
            names: slots,
            len: services.len(),
        })
    }

    /// The Compose profile selecting this stack's optional services. Existing
    /// profile membership (`local-retrieval`, `local-rft`) is preserved, and
    /// `postgres`-only selects none.
    pub(crate) fn profile(&self) -> Option<&'static str> {
        if self.needs_extract() {
            Some("local-rft")
        } else if self
            .services()
            .iter()
            .any(|name| *name == "embed" || *name == "rerank")
        {
            Some("local-retrieval")
        } else {
            None
        }
    }

    /// Does this stack include the `extract` service (and so need the GGUF)?
    pub(crate) fn needs_extract(&self) -> bool {
        self.services().contains(&"extract")
    }

    /// Parse a `--services` value: comma/space-separated allowlisted names,
    /// or one of the profile names (`local-retrieval`, `local-rft`).
    ///
    /// # Errors
    /// An empty value, an unknown name, or an unparseable selection.
    pub(crate) fn parse_selection(raw: &str) -> Result<Self, String> {
        match raw.trim() {
            "local-retrieval" => return Ok(Self::LocalRetrieval),
            "local-rft" => return Ok(Self::LocalRft),
            _ => {}
        }
        let names: Vec<&str> = raw
            .split([',', ' '])
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .collect();
        if names.is_empty() {
            return Err(
                "empty service selection; expected e.g. `--services postgres,embed,rerank`".into(),
            );
        }
        Self::from_services(&names)
    }
}

/// Build the fixed `docker compose` argument shape for one verb. User input
/// never becomes part of a shell string: arguments are fixed to the
/// materialized (checked-in) compose files, the allowlisted profile, and the
/// allowlisted service names.
///
/// `up` and `stop` name the chosen services explicitly at the end (the profile
/// alone still leaves `postgres` in play). `down` and every other verb keep the
/// profile-based shape: `down` takes no service names.
pub(crate) fn compose_args(
    target: ComposeTarget,
    stack: Stack,
    verb: &str,
    extra: &[&str],
) -> Vec<String> {
    let mut args = vec![
        "compose".to_string(),
        "-f".to_string(),
        "compose.yml".to_string(),
        "-f".to_string(),
        target.overlay().to_string(),
    ];
    if let Some(profile) = stack.profile() {
        args.push("--profile".to_string());
        args.push(profile.to_string());
    }
    args.push(verb.to_string());
    args.extend(extra.iter().map(ToString::to_string));
    if matches!(verb, "up" | "stop") {
        args.extend(stack.services().iter().map(ToString::to_string));
    }
    args
}

#[cfg(test)]
#[path = "compose_docker_tests.rs"]
pub(crate) mod tests;
