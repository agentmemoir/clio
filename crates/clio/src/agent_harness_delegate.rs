// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for running a harness's own vendor command to
//! add or remove Clio's MCP server registration, and for reading the harness
//! config file afterwards so the vendor's exit code is never trusted alone.
//! When an add is refused because a registration already exists under
//! another executable, it removes the stale registration and adds it again.
//!
//! ## Owns
//! - The `DelegateEnv` seam: how a harness binary is found and which home
//!   directory holds the harness config, injectable for tests
//! - Spawning the vendor command with the registry's argument list, an
//!   optional fixed confirmation, and capturing its output
//! - The per-vendor exit-code table and the config read-back that decides
//!   the final outcome, including the remove-and-re-add repair of a stale
//!   registration
//! - Removing Clio's own entry directly when a harness ships no remove
//!   command
//!
//! ## Does not own
//! - The harness registry itself, including each harness's config file name
//!   and command builders (see `agent_harness`)
//! - How a config file is parsed, judged, or byte-edited (see
//!   `agent_harness_delegate_read` and `agent_harness_write`)
//! - Printing results or choosing process exit codes; callers do that
//!
//! ## Boundary
//!
//! Changes to how vendor commands run, how their exit codes are read, or how
//! the config file is checked should normally require changes here. Changes
//! to which harnesses exist, what arguments they take, or where their config
//! files live should not.
//!
//! Keep this module focused on the delegate run and its outcome.

use crate::agent_harness::{HarnessRecord, ReadBack, snippet_from_argv};
use crate::agent_harness_delegate_read::{entry_points_at, read_entry};
use crate::agent_harness_write::{RemoveOutcome, WriteError};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// How a harness binary is found: a binary name, and the path to run.
type BinaryLookup = Box<dyn Fn(&str) -> Option<PathBuf>>;

/// Where a harness binary is found and where its config file lives.
pub(crate) struct DelegateEnv {
    lookup: BinaryLookup,
    home: PathBuf,
}

impl DelegateEnv {
    /// The real machine: the binary comes from `PATH`, the config file from
    /// the user's home directory.
    pub(crate) fn production() -> DelegateEnv {
        DelegateEnv {
            lookup: Box::new(lookup_on_path),
            home: std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_default(),
        }
    }

    /// An injected environment, so tests never touch the real `PATH` or the
    /// real home directory. Test-only: production always uses [`production`].
    #[cfg(test)]
    pub(crate) fn with(
        lookup: impl Fn(&str) -> Option<PathBuf> + 'static,
        home: PathBuf,
    ) -> DelegateEnv {
        DelegateEnv {
            lookup: Box::new(lookup),
            home,
        }
    }

    /// The harness's user-scope config file under this home directory. The
    /// file name comes from the registry record, so each harness names its
    /// own file.
    fn config_path(&self, record: &HarnessRecord) -> PathBuf {
        self.home.join(record.config_file)
    }
}

/// Why a delegate run refused or failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DelegateError {
    /// The harness binary could not be found or could not be run.
    BinaryMissing {
        binary: String,
        manual_snippet: String,
    },
    /// The vendor command refused and the config read-back agrees.
    CommandFailed { exit_code: i32, output: String },
    /// The vendor command claimed success but the config read-back says
    /// otherwise, or the config file cannot be checked.
    VerificationFailed { detail: String },
}

/// What a known vendor exit code means. A vendor may exit non-zero for
/// reasons that are not failures, so a non-clean code is only a hint: the
/// read-back decides the outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExitMeaning {
    /// The vendor reported success; the read-back still has to confirm it.
    Clean,
    /// The vendor may be reporting a state we can accept, such as an entry
    /// that is already present or already gone.
    Ambiguous,
}

/// The exit-code meanings every delegate vendor shares: `0` is a clean exit
/// and `1` is ambiguous.
const CLEAN_OR_AMBIGUOUS: &[(i32, ExitMeaning)] =
    &[(0, ExitMeaning::Clean), (1, ExitMeaning::Ambiguous)];

/// Exit code meanings of each delegate vendor, by registry name. A harness
/// absent from the table, or an unknown code, is `Ambiguous`: the read-back
/// decides.
static EXIT_MEANINGS: &[(&str, &[(i32, ExitMeaning)])] = &[
    ("claude", CLEAN_OR_AMBIGUOUS),
    ("codex", CLEAN_OR_AMBIGUOUS),
    ("opencode", CLEAN_OR_AMBIGUOUS),
    ("antigravity", CLEAN_OR_AMBIGUOUS),
    ("hermes", CLEAN_OR_AMBIGUOUS),
];

fn exit_meaning(record: &HarnessRecord, code: i32) -> ExitMeaning {
    EXIT_MEANINGS
        .iter()
        .find(|(name, _)| *name == record.name)
        .and_then(|(_, table)| table.iter().find(|(known, _)| *known == code))
        .map_or(ExitMeaning::Ambiguous, |(_, meaning)| *meaning)
}

/// The fixed confirmation Clio pipes to a vendor that asks for one on add.
/// Hermes asks before it saves, and exits 0 without saving on end of input,
/// so the read-back would reject it. The bytes are constant; no user data is
/// ever sent to the vendor.
fn add_confirmation(record: &HarnessRecord) -> Option<&'static [u8]> {
    match record.name {
        "hermes" => Some(b"y\n"),
        _ => None,
    }
}

/// Which end state a delegate run wants in the config file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Want {
    /// The server entry must be present.
    Present,
    /// The server entry must be gone.
    Absent,
}

/// Run the vendor command that adds the Clio registration, then verify the
/// config file. The message says whether Clio was installed now or was
/// already installed before the call.
pub(crate) fn install(
    record: &HarnessRecord,
    server_name: &str,
    exe: &Path,
    env: &DelegateEnv,
) -> Result<String, DelegateError> {
    run_and_verify(record, server_name, Some(exe), env, Want::Present)
}

/// Run the vendor command that removes the Clio registration, then verify
/// the config file. The message says whether Clio was removed now or was
/// already absent before the call.
pub(crate) fn uninstall(
    record: &HarnessRecord,
    server_name: &str,
    env: &DelegateEnv,
) -> Result<String, DelegateError> {
    run_and_verify(record, server_name, None, env, Want::Absent)
}

fn run_and_verify(
    record: &HarnessRecord,
    server_name: &str,
    exe: Option<&Path>,
    env: &DelegateEnv,
    want: Want,
) -> Result<String, DelegateError> {
    let add_exe = || exe.expect("install always has an executable");
    let argv = match want {
        Want::Present => record.add_argv(server_name, add_exe()),
        Want::Absent => record.remove_argv(server_name),
    };
    // The refusal snippet teaches the manual command for this direction.
    let snippet = match want {
        Want::Present => record.manual_snippet(server_name, add_exe()),
        Want::Absent if argv.is_empty() => format!(
            "remove the `{server_name}` entry from {} by hand",
            env.config_path(record).display()
        ),
        Want::Absent => snippet_from_argv(&argv),
    };
    // A harness with no vendor remove command is edited directly.
    if want == Want::Absent && argv.is_empty() {
        return remove_direct(record, server_name, &snippet, env);
    }
    let stdin = (want == Want::Present)
        .then(|| add_confirmation(record))
        .flatten();
    let (code, output) = run_step(record, env, &argv, &snippet, stdin)?;
    let entry = read_entry(record, &env.config_path(record), server_name, exe)
        .map_err(|detail| DelegateError::VerificationFailed { detail })?;
    let clean = exit_meaning(record, code) == ExitMeaning::Clean;
    match (want, &entry) {
        (Want::Present, Some(value)) if exe.is_some_and(|exe| entry_points_at(value, exe)) => {
            Ok(if clean {
                "installed".to_owned()
            } else {
                "already installed".to_owned()
            })
        }
        (Want::Absent, None) => Ok(if clean {
            "removed".to_owned()
        } else {
            "nothing to remove".to_owned()
        }),
        // The vendor refused a duplicate add, and the existing entry points
        // at another executable. Rewrite it: remove, add again.
        (Want::Present, Some(_)) if !clean => repair_stale(record, server_name, add_exe(), env),
        (Want::Present, _) if clean => Err(DelegateError::VerificationFailed {
            detail: "the vendor command exited 0 but the config file does not register the server under this executable".to_owned(),
        }),
        (Want::Absent, _) if clean => Err(DelegateError::VerificationFailed {
            detail: "the vendor command exited 0 but the config file still registers the server"
                .to_owned(),
        }),
        (_, _) => Err(DelegateError::CommandFailed {
            exit_code: code,
            output,
        }),
    }
}

/// Remove Clio's entry from a JSON config with the shared writer, used for a
/// harness with no vendor remove command, then confirm the entry is gone.
fn remove_direct(
    record: &HarnessRecord,
    server_name: &str,
    snippet: &str,
    env: &DelegateEnv,
) -> Result<String, DelegateError> {
    let ReadBack::Json { key_prefixes } = record.read_back else {
        return Err(DelegateError::VerificationFailed {
            detail: format!(
                "{} has no JSON key path, so Clio cannot remove the entry directly",
                record.name
            ),
        });
    };
    let outcome = crate::agent_harness_write::remove_server_from_file(
        &env.config_path(record),
        key_prefixes,
        server_name,
        snippet,
    )
    .map_err(|err| match err {
        WriteError::Unparseable { path, snippet } => DelegateError::VerificationFailed {
            detail: format!(
                "{} is not valid JSON; remove the entry by hand: {snippet}",
                path.display()
            ),
        },
        WriteError::Io { detail } => DelegateError::VerificationFailed { detail },
    })?;
    let entry = read_entry(record, &env.config_path(record), server_name, None)
        .map_err(|detail| DelegateError::VerificationFailed { detail })?;
    if entry.is_some() {
        return Err(DelegateError::VerificationFailed {
            detail: "the config file still registers the server after the direct edit".to_owned(),
        });
    }
    Ok(match outcome {
        RemoveOutcome::Removed { backup } => format!("removed (backup: {})", backup.display()),
        RemoveOutcome::Absent => "nothing to remove".to_owned(),
    })
}

/// Run one vendor command argv and collect its exit code and output. A
/// binary that cannot be found or run refuses with the manual command for
/// the direction this argv belongs to. A `stdin` confirmation is piped to
/// the vendor, then its input is closed.
fn run_step(
    record: &HarnessRecord,
    env: &DelegateEnv,
    argv: &[OsString],
    snippet: &str,
    stdin: Option<&[u8]>,
) -> Result<(i32, String), DelegateError> {
    let Some(binary) = argv.first() else {
        return Err(DelegateError::VerificationFailed {
            detail: format!("{} has no vendor command to run", record.name),
        });
    };
    let refuse_binary = || DelegateError::BinaryMissing {
        binary: binary.to_string_lossy().into_owned(),
        manual_snippet: snippet.to_owned(),
    };
    let Some(resolved) = binary.to_str().and_then(|name| (env.lookup)(name)) else {
        return Err(refuse_binary());
    };
    run_vendor(&resolved, &argv[1..], stdin).map_err(|_| refuse_binary())
}

/// A registration exists but points at another executable, and the vendor
/// refused the duplicate add. Remove the stale registration, add it again,
/// and verify the read-back now points at this executable. Every step is
/// checked by the config read-back, never by an exit code alone.
fn repair_stale(
    record: &HarnessRecord,
    server_name: &str,
    exe: &Path,
    env: &DelegateEnv,
) -> Result<String, DelegateError> {
    let read = |env: &DelegateEnv| {
        read_entry(record, &env.config_path(record), server_name, Some(exe))
            .map_err(|detail| DelegateError::VerificationFailed { detail })
    };
    let remove_argv = record.remove_argv(server_name);
    if remove_argv.is_empty() {
        // No vendor remove command, such as OpenCode: edit the file directly.
        let snippet = record.manual_snippet(server_name, exe);
        remove_direct(record, server_name, &snippet, env)?;
    } else {
        let remove_snippet = snippet_from_argv(&remove_argv);
        let (remove_code, remove_output) =
            run_step(record, env, &remove_argv, &remove_snippet, None)?;
        if read(env)?.is_some() {
            return Err(DelegateError::CommandFailed {
                exit_code: remove_code,
                output: remove_output,
            });
        }
    }
    let add_argv = record.add_argv(server_name, exe);
    let add_snippet = record.manual_snippet(server_name, exe);
    let confirm = add_confirmation(record);
    let (add_code, add_output) = run_step(record, env, &add_argv, &add_snippet, confirm)?;
    match read(env)? {
        Some(value) if entry_points_at(&value, exe) => Ok("installed".to_owned()),
        Some(_) => Err(DelegateError::VerificationFailed {
            detail: "the vendor command did not rewrite the registration under this executable"
                .to_owned(),
        }),
        None if exit_meaning(record, add_code) == ExitMeaning::Clean => {
            Err(DelegateError::VerificationFailed {
                detail: "the vendor command exited 0 but the config file does not register the server under this executable".to_owned(),
            })
        }
        None => Err(DelegateError::CommandFailed {
            exit_code: add_code,
            output: add_output,
        }),
    }
}

/// Run the vendor command with its registry arguments and collect its
/// output. A `stdin` confirmation, when present, is written and its handle
/// dropped so the vendor sees end of input; writing is best effort and the
/// read-back still decides. Both output streams are drained while it runs,
/// so a chatty vendor cannot block on a full pipe. On overlay filesystems,
/// such as inside containers, a harness binary installed moments ago can
/// briefly report "text file busy", so that one refusal is retried: up to
/// ten spawn attempts with a short sleep between attempts, adding at most
/// 180 ms in the busy case and nothing in the normal case.
fn run_vendor(
    resolved: &Path,
    args: &[OsString],
    stdin: Option<&[u8]>,
) -> std::io::Result<(i32, String)> {
    const BUSY_SPAWN_ATTEMPTS: u32 = 10;
    const BUSY_RETRY_BACKOFF: Duration = Duration::from_millis(20);
    for attempt in 1..=BUSY_SPAWN_ATTEMPTS {
        let spawned = match stdin {
            None => std::process::Command::new(resolved).args(args).output(),
            Some(bytes) => std::process::Command::new(resolved)
                .args(args)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .and_then(|mut child| {
                    use std::io::Write;
                    if let Some(mut handle) = child.stdin.take() {
                        let _ = handle.write_all(bytes);
                    }
                    child.wait_with_output()
                }),
        };
        match spawned {
            Ok(out) => {
                let text = format!(
                    "{}{}",
                    String::from_utf8_lossy(&out.stdout),
                    String::from_utf8_lossy(&out.stderr)
                );
                return Ok((out.status.code().unwrap_or(1), text));
            }
            Err(err)
                if err.kind() == std::io::ErrorKind::ExecutableFileBusy
                    && attempt < BUSY_SPAWN_ATTEMPTS =>
            {
                std::thread::sleep(BUSY_RETRY_BACKOFF);
            }
            Err(err) => return Err(err),
        }
    }
    unreachable!("the loop returns or errors on its last attempt");
}

/// Find a binary on the process `PATH`: a file the user could run.
fn lookup_on_path(binary: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(binary))
        .find(|candidate| is_runnable(candidate))
}

#[cfg(unix)]
fn is_runnable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.is_file()
        && std::fs::metadata(path).is_ok_and(|meta| meta.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_runnable(path: &Path) -> bool {
    path.is_file()
}

#[cfg(test)]
#[path = "agent_harness_delegate_test_support_tests.rs"]
pub(crate) mod test_support_tests;

#[cfg(test)]
#[path = "agent_harness_delegate_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "agent_harness_delegate_harness_tests.rs"]
mod harness_tests;
