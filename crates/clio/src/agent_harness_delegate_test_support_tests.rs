// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is the shared harness of the delegate runner tests: the stub
//! vendor executables, the fixture directories, and the recorded-argument
//! helpers. It holds no scenario and no assertion; the sibling test modules
//! own those.
//!
//! ## Owns
//! - `Fixture`: unique temp directories, the per-record config writers in
//!   each harness's own config shape, and the flat and vendor-like stubs
//! - The exec directory resolver the stubs are written into, and its probe
//! - `record`, `record_named`, and `expected_args`
//!
//! ## Does not own
//! - Any scenario or assertion (the test modules own those)
//! - The delegate implementation under test (`agent_harness_delegate`)
//!
//! ## Boundary
//!
//! Changes to how the shared stubs and fixtures are built should normally
//! require changes here. Changes to a scenario's outcome assertions should not.
//!
//! Keep this module focused on the shared test seams.

use super::*;
use crate::agent_harness::{SERVER_NAME, resolve};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

#[cfg(unix)]
use std::process::Command;

/// Nanoseconds plus a process-global counter: two calls can never get the
/// same suffix, even when they read the clock in the same nanosecond.
static DIR_SEQ: AtomicU64 = AtomicU64::new(0);

fn unique_suffix() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    format!("{nanos}-{}", DIR_SEQ.fetch_add(1, Ordering::Relaxed))
}

pub(crate) fn record() -> &'static HarnessRecord {
    record_named("claude")
}

/// The implemented record for a registry name, or a panic naming it.
pub(crate) fn record_named(name: &str) -> &'static HarnessRecord {
    resolve(name).unwrap_or_else(|| panic!("{name} must resolve"))
}

/// Whether `dir` can run a program at all: write a trivial script there,
/// make it `0755`, start it, and remove it. Mode bits and mount options
/// describe policy they do not enforce, so only the start answers this.
#[cfg(unix)]
fn can_execute(dir: &Path) -> bool {
    let probe = dir.join(format!("exec-probe-{}", unique_suffix()));
    let prepared = fs::write(&probe, "#!/bin/sh\nexit 0\n")
        .and_then(|()| fs::set_permissions(&probe, fs::Permissions::from_mode(0o755)));
    let ran = prepared.is_ok_and(|()| {
        Command::new(&probe)
            .output()
            .is_ok_and(|out| out.status.success())
    });
    let _ = fs::remove_file(&probe);
    ran
}

/// Where stub executables are created: `NOEXEC_EXEC_DIR` when set, otherwise
/// the directory of this test binary, which cargo owns and which therefore
/// permits exec. There is no fallback to a scratch directory, because that
/// is exactly the placement a program cannot be started from.
#[cfg(unix)]
fn exec_dir_base() -> Result<PathBuf, String> {
    if let Some(dir) = std::env::var_os("NOEXEC_EXEC_DIR") {
        return Ok(PathBuf::from(dir));
    }
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
        .ok_or_else(|| {
            "this test binary has no directory to run stubs from; set NOEXEC_EXEC_DIR".to_owned()
        })
}

/// A per-test directory under `base`, named with this process and a unique
/// suffix, created and then checked by `runs`. A base that refuses the probe
/// is an error naming the directory and the remedy, never a silent fallback.
#[cfg(unix)]
fn exec_dir_in(base: &Path, label: &str, runs: impl Fn(&Path) -> bool) -> Result<PathBuf, String> {
    let dir = base.join(format!(
        "clio-agent-harness-delegate-{label}-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    fs::create_dir_all(&dir).map_err(|err| format!("{}: {err}", dir.display()))?;
    if !runs(&dir) {
        return Err(format!(
            "{} refuses exec, so no stub can run there; set NOEXEC_EXEC_DIR to a directory that allows it",
            dir.display()
        ));
    }
    Ok(dir)
}

/// The directory for one test's stub executables; see [`exec_dir_in`].
#[cfg(unix)]
fn exec_dir(label: &str) -> Result<PathBuf, String> {
    exec_dir_in(&exec_dir_base()?, label, can_execute)
}

/// The stub's redirection target, as a shell word: single-quoted, with an
/// embedded single quote written as `'\''`, so a temp directory containing
/// spaces or shell metacharacters cannot break the script.
#[cfg(unix)]
fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
}

/// The JSON server entry every JSON harness stores for `exe`.
fn json_entry(exe: &Path) -> serde_json::Value {
    serde_json::json!({
        "type": "stdio",
        "command": exe.to_string_lossy().into_owned(),
        "args": ["mcp", "stdio"],
        "env": {}
    })
}

/// The config text this record's vendor writes for `exe`, in that harness's
/// own shape, so the read-back accepts it.
fn config_text(record: &HarnessRecord, exe: &Path) -> String {
    match record.name {
        "codex" => format!(
            "[mcp_servers.{SERVER_NAME}]\ncommand = \"{exe}\"\nargs = [\"mcp\", \"stdio\"]\n",
            exe = exe.to_string_lossy()
        ),
        "hermes" => format!(
            "mcp_servers:\n  {SERVER_NAME}:\n    command: {exe}\n    args:\n      - mcp\n      - stdio\n",
            exe = exe.to_string_lossy()
        ),
        "opencode" => serde_json::json!({
            "mcp": { "servers": { "clio": json_entry(exe) } }
        })
        .to_string(),
        _ => serde_json::json!({
            "mcpServers": { "clio": json_entry(exe) }
        })
        .to_string(),
    }
}

/// The vendor command name of a delegate record: the first element of its
/// add argv, which is the binary the injected lookup is asked for.
#[cfg(unix)]
fn vendor_binary(record: &HarnessRecord) -> String {
    record
        .add_argv(SERVER_NAME, Path::new("clio"))
        .first()
        .map(|arg| arg.to_string_lossy().into_owned())
        .expect("a delegate record builds an add argv")
}

/// A unique directory, removed when the guard drops. Files the stub only
/// writes live in `root`; the stub executable itself lives in `exec`.
pub(crate) struct Fixture {
    pub(crate) root: PathBuf,
    #[cfg(unix)]
    exec: Option<PathBuf>,
}

impl Fixture {
    pub(crate) fn new(label: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "clio-agent-harness-delegate-{label}-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        fs::create_dir_all(&root).expect("fixture directory can be created");
        Self {
            root,
            #[cfg(unix)]
            exec: None,
        }
    }

    fn home(&self) -> PathBuf {
        self.root.join("home")
    }

    pub(crate) fn exe(&self) -> PathBuf {
        self.root.join("clio")
    }

    /// The config file this record's harness owns under this fixture's home.
    pub(crate) fn config_path(&self, record: &HarnessRecord) -> PathBuf {
        self.home().join(record.config_file)
    }

    /// The directory for this test's stub executable, resolved once and
    /// removed on drop.
    #[cfg(unix)]
    fn exec(&mut self) -> Result<PathBuf, String> {
        if let Some(dir) = &self.exec {
            Ok(dir.clone())
        } else {
            let dir = exec_dir("stub")?;
            self.exec = Some(dir.clone());
            Ok(dir)
        }
    }

    /// Write a stub named `binary` and build an environment that finds it.
    /// `Err` when no directory that permits exec exists, so a scenario that
    /// needs a stub fails with the reason instead of passing without one.
    #[cfg(unix)]
    fn env_with_stub(&mut self, binary: &str, script: String) -> Result<DelegateEnv, String> {
        let exec = self.exec()?;
        let path = exec.join(binary);
        fs::write(&path, script).expect("stub script can be written");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755))
            .expect("stub script can be made executable");
        Ok(DelegateEnv::with(
            move |name: &str| Some(exec.join(name)),
            self.home(),
        ))
    }

    /// A stub for `record`'s vendor binary that records the arguments it
    /// received and exits with `code`.
    #[cfg(unix)]
    pub(crate) fn stub_env_for(
        &mut self,
        record: &HarnessRecord,
        code: i32,
    ) -> Result<DelegateEnv, String> {
        let binary = vendor_binary(record);
        let script = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > {}\nexit {code}\n",
            shell_quote(&self.root.join("args.txt"))
        );
        self.env_with_stub(&binary, script)
    }

    /// A stub `claude` that records the arguments it received and exits
    /// with `code`.
    #[cfg(unix)]
    pub(crate) fn stub_env(&mut self, code: i32) -> Result<DelegateEnv, String> {
        self.stub_env_for(record(), code)
    }

    /// The vendor-like stub script for `record`: `add` refuses with exit 1
    /// when the config file already exists, otherwise it writes that
    /// harness's config shape itself and exits 0; `remove` deletes the config
    /// file and exits 0. When `reads_stdin` is set the stub first drains
    /// stdin into `stdin.txt`, so a test can assert a piped confirmation
    /// arrived. Every call appends its argv to the record.
    #[cfg(unix)]
    fn vendor_stub_script(&self, record: &HarnessRecord, reads_stdin: bool) -> String {
        let config_path = self.config_path(record);
        let config = shell_quote(&config_path);
        let config_dir = shell_quote(
            &config_path
                .parent()
                .map_or_else(|| self.home(), Path::to_path_buf),
        );
        let args = shell_quote(&self.root.join("args.txt"));
        let contents = config_text(record, &self.exe());
        let drain_stdin = if reads_stdin {
            format!(
                "command -p cat > {}\n  ",
                shell_quote(&self.root.join("stdin.txt"))
            )
        } else {
            String::new()
        };
        format!(
            "#!/bin/sh\nmkdir -p {config_dir}\nprintf '%s\\n' \"$@\" >> {args}\ncase \"$2\" in\nadd)\n  {drain_stdin}if [ -f {config} ]; then exit 1; fi\n  command -p cat > {config} <<'CLIO_CONFIG'\n{contents}\nCLIO_CONFIG\n  exit 0 ;;\nremove)\n  rm -f {config}\n  exit 0 ;;\nesac\nexit 1\n"
        )
    }

    /// A vendor-like stub for `record`; see [`Fixture::vendor_stub_script`].
    #[cfg(unix)]
    pub(crate) fn stub_env_vendor_for(
        &mut self,
        record: &HarnessRecord,
        reads_stdin: bool,
    ) -> Result<DelegateEnv, String> {
        let binary = vendor_binary(record);
        let script = self.vendor_stub_script(record, reads_stdin);
        self.env_with_stub(&binary, script)
    }

    /// A vendor-like stub `claude`; see [`Fixture::stub_env_vendor_for`].
    #[cfg(unix)]
    pub(crate) fn stub_env_vendor(&mut self) -> Result<DelegateEnv, String> {
        self.stub_env_vendor_for(record(), false)
    }

    /// A stub for `record` that records its arguments and its stdin, writes
    /// nothing, and exits with `code`. It models a vendor that claims success
    /// without saving the config.
    #[cfg(unix)]
    pub(crate) fn stub_env_reads_stdin(
        &mut self,
        record: &HarnessRecord,
        code: i32,
    ) -> Result<DelegateEnv, String> {
        let binary = vendor_binary(record);
        let script = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > {}\ncommand -p cat > {}\nexit {code}\n",
            shell_quote(&self.root.join("args.txt")),
            shell_quote(&self.root.join("stdin.txt")),
        );
        self.env_with_stub(&binary, script)
    }

    /// An environment where the harness binary is nowhere to be found.
    pub(crate) fn empty_env(&self) -> DelegateEnv {
        DelegateEnv::with(|_| None, self.home())
    }

    /// An environment with a custom binary lookup and this fixture's home,
    /// for lookups that resolve to something that cannot be run.
    pub(crate) fn env_with_lookup(
        &self,
        lookup: impl Fn(&str) -> Option<PathBuf> + 'static,
    ) -> DelegateEnv {
        DelegateEnv::with(lookup, self.home())
    }

    /// Write `contents` verbatim as `record`'s config file, creating the
    /// parent directories.
    pub(crate) fn write_raw(&self, record: &HarnessRecord, contents: &str) -> PathBuf {
        let path = self.config_path(record);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("config directory can be created");
        }
        fs::write(&path, contents).expect("config can be written");
        path
    }

    /// A config in `record`'s shape that registers the server under `command`.
    pub(crate) fn write_config_for(&self, record: &HarnessRecord, command: &Path) -> PathBuf {
        self.write_raw(record, &config_text(record, command))
    }

    /// A Claude config that registers the server under `command`.
    pub(crate) fn write_config(&self, command: &Path) -> PathBuf {
        self.write_config_for(record(), command)
    }

    /// An unparseable config for `record`.
    pub(crate) fn write_garbage_config_for(&self, record: &HarnessRecord) {
        self.write_raw(record, "not json at all");
    }

    pub(crate) fn write_garbage_config(&self) {
        self.write_garbage_config_for(record());
    }

    /// The argument vector the stub received, without its own name.
    #[cfg(unix)]
    pub(crate) fn recorded_args(&self) -> Vec<String> {
        fs::read_to_string(self.root.join("args.txt"))
            .expect("the stub recorded its arguments")
            .lines()
            .map(str::to_owned)
            .collect()
    }

    /// The bytes the stub read from stdin.
    #[cfg(unix)]
    pub(crate) fn recorded_stdin(&self) -> String {
        fs::read_to_string(self.root.join("stdin.txt")).expect("the stub recorded its stdin")
    }

    /// Every `.clio-backup-` sibling of the config file, sorted. One test's
    /// fixture owns its directory, so no other test shares it.
    pub(crate) fn backup_siblings(&self, record: &HarnessRecord) -> Vec<PathBuf> {
        let Some(dir) = self.config_path(record).parent().map(Path::to_path_buf) else {
            return Vec::new();
        };
        let mut found: Vec<PathBuf> = fs::read_dir(dir)
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.contains(".clio-backup-"))
            })
            .collect();
        found.sort();
        found
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
        #[cfg(unix)]
        if let Some(exec) = &self.exec {
            let _ = fs::remove_dir_all(exec);
        }
    }
}

/// The argument vector the stub should have received, without argv[0]:
/// argv[0] is the vendor command itself.
pub(crate) fn expected_args(argv: &[std::ffi::OsString]) -> Vec<String> {
    argv.iter()
        .skip(1)
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect()
}

/// A base that cannot run the probe is an error, never a fallback to a
/// directory that will refuse the stub later. The injected check stands in
/// for a `noexec` mount, which a test cannot create for itself.
#[cfg(unix)]
#[test]
fn exec_dir_refuses_a_directory_that_cannot_run_its_probe() {
    let fixture = Fixture::new("refusing-exec");
    let err = exec_dir_in(&fixture.root, "refusing", |_| false)
        .expect_err("a directory that cannot run its probe must be refused");
    assert!(
        err.contains("refuses exec"),
        "the refusal names the cause: {err}"
    );
    assert!(
        err.contains("NOEXEC_EXEC_DIR"),
        "the refusal names the remedy: {err}"
    );
}
