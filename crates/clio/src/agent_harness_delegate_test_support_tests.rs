// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! The shared harness of the delegate runner tests: stub executables, fixture
//! directories, and recorded-argument helpers; no scenario and no assertion.
//! How these seams are built belongs here; scenario outcomes do not.
//!
//! ## Owns
//! - `Fixture`: temp dirs, config writers, the stubs, the exec resolver and probe
//! - `record`, `record_named`, and `expected_args`
//!
//! ## Does not own
//! - Any scenario or assertion, and `agent_harness_delegate` itself

use super::*;
use crate::agent_harness::{SERVER_NAME, resolve};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::{ffi::OsString, fs};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

#[cfg(unix)]
use std::process::Command;

/// Nanoseconds plus a counter: two calls never share a suffix.
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

/// Whether `dir` can run a program: write a `0755` script, start it, remove it.
#[cfg(unix)]
fn can_execute(dir: &Path) -> bool {
    let probe = dir.join(format!("exec-probe-{}", unique_suffix()));
    let prepared = fs::write(&probe, "#!/bin/sh\nexit 0\n")
        .and_then(|()| fs::set_permissions(&probe, fs::Permissions::from_mode(0o755)));
    let ran = prepared.is_ok_and(|()| {
        // ETXTBSY (26): a concurrent fork holds the write fd; retry, others final.
        for attempt in 0..20 {
            match Command::new(&probe).output() {
                Ok(out) => return out.status.success(),
                Err(err) if err.raw_os_error() == Some(26) && attempt < 19 => {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                Err(_) => return false,
            }
        }
        false
    });
    let _ = fs::remove_file(&probe);
    ran
}

/// Stub home: `NOEXEC_EXEC_DIR` when set, else this binary's dir; never scratch.
#[cfg(unix)]
fn exec_dir_base() -> Result<PathBuf, String> {
    exec_dir_base_from(|name| std::env::var_os(name))
}

/// [`exec_dir_base`] with `lookup` supplying the environment; no process mutation.
#[cfg(unix)]
fn exec_dir_base_from(lookup: impl Fn(&str) -> Option<OsString>) -> Result<PathBuf, String> {
    if let Some(dir) = lookup("NOEXEC_EXEC_DIR") {
        return Ok(PathBuf::from(dir));
    }
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
        .ok_or_else(|| {
            "this test binary has no directory to run stubs from; set NOEXEC_EXEC_DIR".to_owned()
        })
}

/// A per-test directory under `base`, created then probed by `runs`; a refusal
/// names the directory and the remedy. Never scratch storage, which cannot exec.
#[cfg(unix)]
fn exec_dir_in(base: &Path, label: &str, runs: impl Fn(&Path) -> bool) -> Result<PathBuf, String> {
    let dir = base.join(format!(
        "clio-agent-harness-delegate-{label}-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    fs::create_dir_all(&dir).map_err(|err| format!("{}: {err}", dir.display()))?;
    if !runs(&dir) {
        let _ = fs::remove_dir_all(&dir);
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

/// The stub's redirection target as a shell word: single-quoted, with an
/// embedded quote written `'\''`, so spaces and metacharacters cannot break it.
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

/// The config text this record's vendor writes for `exe`, in its own shape.
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

/// The vendor command of a delegate record: the first element of its add argv.
#[cfg(unix)]
fn vendor_binary(record: &HarnessRecord) -> String {
    record
        .add_argv(SERVER_NAME, Path::new("clio"))
        .first()
        .map(|arg| arg.to_string_lossy().into_owned())
        .expect("a delegate record builds an add argv")
}

/// A unique directory, removed on drop: files live in `root`, the stub in `exec`.
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

    /// This test's stub directory, resolved once and removed on drop.
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

    /// Write a stub named `binary` and an environment that finds it; `Err` when no
    /// directory permits exec, so a stub-free scenario fails loudly.
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

    /// A stub for `record`'s vendor binary: records its arguments, exits `code`.
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

    /// A stub `claude` that records its arguments and exits with `code`.
    #[cfg(unix)]
    pub(crate) fn stub_env(&mut self, code: i32) -> Result<DelegateEnv, String> {
        self.stub_env_for(record(), code)
    }

    /// The vendor-like stub for `record`: `add` writes that harness's own config
    /// shape (exit 1 if it exists), `remove` deletes it, argv and stdin recorded.
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

    /// A stub for `record` that records argv and stdin, writes nothing, exits `code`.
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

    /// An environment with a custom lookup and this fixture's home.
    pub(crate) fn env_with_lookup(
        &self,
        lookup: impl Fn(&str) -> Option<PathBuf> + 'static,
    ) -> DelegateEnv {
        DelegateEnv::with(lookup, self.home())
    }

    /// Write `contents` verbatim as `record`'s config file, creating parents.
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

    /// Every `.clio-backup-` sibling of the config file, sorted.
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

/// The argument vector the stub should have received, without argv[0].
pub(crate) fn expected_args(argv: &[std::ffi::OsString]) -> Vec<String> {
    argv.iter()
        .skip(1)
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect()
}

/// A base that cannot run the probe is an error, never a fallback to a
/// directory that will refuse the stub later: a `noexec` mount, injected.
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

#[cfg(unix)]
#[test]
fn exec_dir_base_returns_the_configured_override() {
    let fixture = Fixture::new("configured-exec");
    let base = exec_dir_base_from(|name| {
        (name == "NOEXEC_EXEC_DIR").then(|| fixture.root.clone().into_os_string())
    })
    .expect("a configured override is the base");
    assert_eq!(base, fixture.root);
}

#[cfg(unix)]
#[test]
fn exec_dir_resolves_a_directory_that_runs_the_probe() {
    let dir = exec_dir("resolver-positive").expect("the real base runs the probe");
    assert!(dir.is_dir(), "the resolver created {}", dir.display());
    fs::remove_dir_all(&dir).expect("the resolved directory is removed");
}

#[cfg(unix)]
#[test]
fn can_execute_refuses_a_directory_that_does_not_exist() {
    let absent = Fixture::new("absent-exec").root.join("absent");
    assert!(!can_execute(&absent), "no exec in {}", absent.display());
}
