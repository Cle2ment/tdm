//! End-to-end tests for the `tdmm` binary: clap parsing shapes, `init`
//! skeleton bootstrap (idempotency / --force), `call` through the runtime
//! engine (exit-code contract, default provider selection, caching), and
//! the M1 surface (`use`, `keys`, `doctor`, `logs`, `stats`, `config`).
//!
//! Every test that touches config state scopes `TDM_CONFIG_DIR` to a
//! tempdir; tests that may write the audit db use a config whose
//! `[audit].path` lives inside that tempdir too.

use std::fs;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use clap::Parser;
use predicates::str::contains;
use tdm_core::DecisionResult;
use tdmm::cli::Cli;
use tdmm::cli::{Command as Subcommand, ProviderArg, ProviderArg as PA};

/// Valid DecisionRequest exercising all three primitives.
const VALID_REQUEST: &str = r#"{
  "state": { "repo": "tdm", "filesChanged": 3, "danger": 7 },
  "questions": [
    { "id": "risk", "instructions": "Rate the merge risk.",
      "primitive": { "type": "score", "levels": ["low", "mid", "high"] } },
    { "id": "proceed", "instructions": "Proceed without human review?",
      "primitive": { "type": "noul" } },
    { "id": "branch", "instructions": "Which branch strategy?",
      "primitive": { "type": "choice", "options": ["main", "feature"] } }
  ]
}"#;

/// A `tdmm` command with key env vars stripped (deterministic defaults);
/// call sites add `TDM_CONFIG_DIR` etc. as needed.
fn tdmm() -> Command {
    let mut cmd = Command::cargo_bin("tdmm").expect("tdmm binary must be built");
    cmd.env_remove("TDM_JEV_API_KEY")
        .env_remove("TYPESAFE_API_KEY")
        .env_remove("TDM_PROVIDER")
        .env_remove("TDM_CONFIG_DIR");
    cmd
}

fn stdout_text(assert: &assert_cmd::assert::Assert) -> String {
    String::from_utf8(assert.get_output().stdout.clone()).expect("stdout must be utf-8")
}

fn stderr_text(assert: &assert_cmd::assert::Assert) -> String {
    String::from_utf8(assert.get_output().stderr.clone()).expect("stderr must be utf-8")
}

/// Writes a minimal hermetic config.toml into `dir`: mock-only, default
/// `mock`, audit db inside `dir` (literal TOML string keeps Windows
/// backslashes intact).
fn write_mock_config(dir: &Path) {
    let audit = dir.join("audit.db");
    fs::write(
        dir.join("config.toml"),
        format!(
            "version = 1\n\n[provider.mock]\ntype = \"mock\"\n\n\
             [defaults]\nprovider = \"mock\"\n\n[audit]\npath = '{}'\n",
            audit.display()
        ),
    )
    .unwrap();
}

/// An isolated config dir with a mock-only config (audit db inside).
fn isolated_dir() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    write_mock_config(dir.path());
    dir
}

fn tdmm_in(dir: &Path) -> Command {
    let mut cmd = tdmm();
    cmd.env("TDM_CONFIG_DIR", dir);
    cmd
}

// ---------------------------------------------------------------------------
// 1. clap parsing
// ---------------------------------------------------------------------------

#[test]
fn parses_init_with_and_without_force() {
    let cli = Cli::try_parse_from(["tdmm", "init"]).unwrap();
    assert!(matches!(cli.command, Subcommand::Init { force: false }));

    let cli = Cli::try_parse_from(["tdmm", "init", "--force"]).unwrap();
    assert!(matches!(cli.command, Subcommand::Init { force: true }));
}

#[test]
fn parses_call_shapes() {
    let cli = Cli::try_parse_from(["tdmm", "call"]).unwrap();
    let Subcommand::Call {
        file,
        provider,
        compact,
    } = cli.command
    else {
        panic!("expected call subcommand");
    };
    assert_eq!(
        (file.is_none(), provider.is_none(), compact),
        (true, true, false),
        "bare `call` = stdin, env-selected provider, pretty output"
    );

    let cli = Cli::try_parse_from(["tdmm", "call", "req.json"]).unwrap();
    let Subcommand::Call { file, .. } = cli.command else {
        panic!("expected call subcommand");
    };
    assert_eq!(file, Some(PathBuf::from("req.json")));

    let cli = Cli::try_parse_from(["tdmm", "call", "-"]).unwrap();
    let Subcommand::Call { file, .. } = cli.command else {
        panic!("expected call subcommand");
    };
    assert_eq!(file, Some(PathBuf::from("-")), "bare dash = explicit stdin");

    let cli = Cli::try_parse_from([
        "tdmm",
        "call",
        "--provider",
        "jev",
        "--compact",
        "batch.json",
    ])
    .unwrap();
    let Subcommand::Call {
        file,
        provider,
        compact,
    } = cli.command
    else {
        panic!("expected call subcommand");
    };
    assert_eq!(file, Some(PathBuf::from("batch.json")));
    assert_eq!(provider, Some(PA::Jev));
    assert!(compact);

    let cli = Cli::try_parse_from(["tdmm", "call", "--provider", "mock"]).unwrap();
    let Subcommand::Call { provider, .. } = cli.command else {
        panic!("expected call subcommand");
    };
    assert_eq!(provider, Some(ProviderArg::Mock));
}

#[test]
fn parses_new_m1_subcommands() {
    let cli = Cli::try_parse_from(["tdmm", "use", "mock"]).unwrap();
    assert!(matches!(cli.command, Subcommand::Use { provider } if provider == "mock"));

    let cli = Cli::try_parse_from(["tdmm", "keys", "set", "jev", "--stdin"]).unwrap();
    let Subcommand::Keys { command } = cli.command else {
        panic!("expected keys subcommand");
    };
    assert!(matches!(
        command,
        tdmm::cli::KeysCommand::Set { ref provider, key: None, stdin: true } if provider == "jev"
    ));

    let cli = Cli::try_parse_from(["tdmm", "keys", "list"]).unwrap();
    assert!(matches!(
        cli.command,
        Subcommand::Keys {
            command: tdmm::cli::KeysCommand::List
        }
    ));

    let cli = Cli::try_parse_from(["tdmm", "keys", "rm", "jev"]).unwrap();
    assert!(matches!(
        cli.command,
        Subcommand::Keys {
            command: tdmm::cli::KeysCommand::Rm { .. }
        }
    ));

    let cli = Cli::try_parse_from(["tdmm", "doctor", "--json"]).unwrap();
    assert!(matches!(cli.command, Subcommand::Doctor { json: true }));

    let cli = Cli::try_parse_from([
        "tdmm",
        "logs",
        "--session",
        "s1",
        "--harness",
        "tdmm",
        "--provider",
        "mock",
        "--limit",
        "5",
        "--json",
    ])
    .unwrap();
    let Subcommand::Logs {
        session,
        harness,
        provider,
        limit,
        json,
    } = cli.command
    else {
        panic!("expected logs subcommand");
    };
    assert_eq!(
        (
            session.as_deref(),
            harness.as_deref(),
            provider.as_deref(),
            limit,
            json
        ),
        (Some("s1"), Some("tdmm"), Some("mock"), 5, true)
    );

    let cli = Cli::try_parse_from(["tdmm", "stats", "--json"]).unwrap();
    assert!(matches!(cli.command, Subcommand::Stats { json: true }));

    for shape in [["config", "path"], ["config", "validate"]] {
        let cli = Cli::try_parse_from(["tdmm"].iter().chain(shape.iter()).copied()).unwrap();
        assert!(matches!(cli.command, Subcommand::Config { .. }));
    }
}

#[test]
fn rejects_invalid_cli_shapes() {
    // No subcommand.
    assert!(Cli::try_parse_from(["tdmm"]).is_err());
    // Unknown subcommand.
    assert!(Cli::try_parse_from(["tdmm", "frobnicate"]).is_err());
    // Unknown provider value.
    assert!(Cli::try_parse_from(["tdmm", "call", "--provider", "startlux"]).is_err());
    // Unknown flag.
    assert!(Cli::try_parse_from(["tdmm", "init", "--clobber"]).is_err());
    // Unknown keys subcommand.
    assert!(Cli::try_parse_from(["tdmm", "keys", "rotate", "jev"]).is_err());
}

// ---------------------------------------------------------------------------
// 2. init
// ---------------------------------------------------------------------------

#[test]
fn init_creates_both_skeleton_files() {
    let dir = tempfile::tempdir().unwrap();
    let stdout = tdmm()
        .env("TDM_CONFIG_DIR", dir.path())
        .args(["init"])
        .assert()
        .success();
    let stdout = stdout_text(&stdout);
    assert!(stdout.contains("config.toml"), "stdout names created paths");
    assert!(stdout.contains("auth.toml"), "stdout names created paths");

    let config = fs::read_to_string(dir.path().join("config.toml")).unwrap();
    assert!(config.contains("version = 1"));
    assert!(config.contains("[provider.jev]"));
    assert!(config.contains("[defaults]"));
    assert!(config.contains("confidence_floor = 0.55"));
    assert!(config.contains("[cache]"));
    assert!(config.contains("[retry]"));
    assert!(config.contains("[audit]"));
    // startlux stays a commented placeholder.
    assert!(config.contains("# [provider.startlux]"));

    let auth = fs::read_to_string(dir.path().join("auth.toml")).unwrap();
    assert!(auth.contains("[jev]"));
    assert!(auth.contains("api_key = \"\""));
    assert!(auth.contains("TDM_JEV_API_KEY"));
    assert!(auth.contains("TYPESAFE_API_KEY"));
}

#[test]
fn init_refuses_overwrite_without_force() {
    let dir = tempfile::tempdir().unwrap();
    tdmm()
        .env("TDM_CONFIG_DIR", dir.path())
        .arg("init")
        .assert()
        .success();

    let config_path = dir.path().join("config.toml");
    let before = fs::read_to_string(&config_path).unwrap();

    let assert = tdmm()
        .env("TDM_CONFIG_DIR", dir.path())
        .arg("init")
        .assert()
        .failure();
    assert!(
        assert
            .get_output()
            .status
            .code()
            .is_some_and(|code| code != 0),
        "rerun must fail"
    );
    assert.stderr(contains("--force"));

    // Untouched on refusal.
    let after = fs::read_to_string(&config_path).unwrap();
    assert_eq!(after, before);
}

#[test]
fn init_force_overwrites() {
    let dir = tempfile::tempdir().unwrap();
    tdmm()
        .env("TDM_CONFIG_DIR", dir.path())
        .arg("init")
        .assert()
        .success();

    let config_path = dir.path().join("config.toml");
    fs::write(&config_path, "# hand-edited marker\n").unwrap();

    tdmm()
        .env("TDM_CONFIG_DIR", dir.path())
        .args(["init", "--force"])
        .assert()
        .success();

    let config = fs::read_to_string(&config_path).unwrap();
    assert!(!config.contains("hand-edited marker"));
    assert!(config.contains("[provider.jev]"));
}

// ---------------------------------------------------------------------------
// 3. call end-to-end (mock, through the runtime engine)
// ---------------------------------------------------------------------------

#[test]
fn call_mock_via_stdin_answers_every_question() {
    let dir = isolated_dir();
    let assert = tdmm_in(dir.path())
        .args(["call", "--provider", "mock"])
        .write_stdin(VALID_REQUEST)
        .assert()
        .success();

    let stdout = stdout_text(&assert);
    let result: DecisionResult = serde_json::from_str(&stdout).expect("pretty JSON on stdout");
    let ids: Vec<&str> = result.answers.iter().map(|a| a.id.as_str()).collect();
    assert_eq!(ids, ["risk", "proceed", "branch"], "ids echoed in order");
    assert_eq!(result.provider.id, "mock");
    // Shape only: mock usage is synthetic, nothing further to assert.
    let _ = result.usage;
    // Pretty by default.
    assert!(stdout.contains('\n'), "default output is multi-line");
}

#[test]
fn call_mock_via_file_argument() {
    let dir = isolated_dir();
    let request_path = dir.path().join("request.json");
    fs::write(&request_path, VALID_REQUEST).unwrap();

    let assert = tdmm_in(dir.path())
        .args(["call", "--provider", "mock"])
        .arg(&request_path)
        .assert()
        .success();

    let result: DecisionResult =
        serde_json::from_str(&stdout_text(&assert)).expect("valid DecisionResult JSON");
    assert_eq!(result.answers.len(), 3);
}

#[test]
fn call_compact_emits_single_line_json() {
    let dir = isolated_dir();
    let assert = tdmm_in(dir.path())
        .args(["call", "--provider", "mock", "--compact"])
        .write_stdin(VALID_REQUEST)
        .assert()
        .success();

    let stdout = stdout_text(&assert);
    assert_eq!(stdout.trim_end().lines().count(), 1, "--compact = one line");
    let result: DecisionResult = serde_json::from_str(&stdout).expect("compact JSON parses");
    assert_eq!(result.provider.id, "mock");
}

#[test]
fn call_malformed_json_exits_2() {
    let assert = tdmm_in(isolated_dir().path())
        .args(["call", "--provider", "mock"])
        .write_stdin("{ definitely not json")
        .assert()
        .failure();

    let output = assert.get_output();
    assert_eq!(output.status.code(), Some(2), "serde failure -> exit 2");
    assert!(output.stdout.is_empty(), "no result on stdout");
    assert!(!output.stderr.is_empty(), "diagnosis on stderr");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("DecisionRequest"), "error names the type");
}

#[test]
fn call_invalid_question_shape_exits_2() {
    // Choice with zero options violates tdm_core::Question::validate.
    let bad = r#"{ "state": null, "questions": [ { "id": "bad",
        "instructions": "pick one", "primitive": { "type": "choice", "options": [] } } ] }"#;
    let assert = tdmm_in(isolated_dir().path())
        .args(["call", "--provider", "mock"])
        .write_stdin(bad)
        .assert()
        .failure();

    let output = assert.get_output();
    assert_eq!(output.status.code(), Some(2), "validation -> exit 2");
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("bad"), "error names the offending question");
}

// ---------------------------------------------------------------------------
// 4. default provider selection
// ---------------------------------------------------------------------------

#[test]
fn call_defaults_to_mock_without_key_env() {
    // Key vars are stripped by the `tdmm()` helper; the isolated config
    // sets [defaults].provider = mock, so the call must be an offline
    // success, never jev (which would fail on the missing key).
    let dir = isolated_dir();
    let assert = tdmm_in(dir.path())
        .arg("call")
        .write_stdin(VALID_REQUEST)
        .assert()
        .success();

    let result: DecisionResult =
        serde_json::from_str(&stdout_text(&assert)).expect("valid DecisionResult JSON");
    assert_eq!(result.provider.id, "mock", "configured default is mock");
}

#[test]
fn explicit_mock_beats_present_key_env() {
    let dir = isolated_dir();
    let assert = tdmm_in(dir.path())
        .args(["call", "--provider", "mock"])
        .env("TDM_JEV_API_KEY", "bogus-key-must-not-matter")
        .write_stdin(VALID_REQUEST)
        .assert()
        .success();

    let stdout = stdout_text(&assert);
    assert!(
        !stdout.contains("bogus-key-must-not-matter"),
        "keys never printed"
    );
    let result: DecisionResult = serde_json::from_str(&stdout).unwrap();
    assert_eq!(result.provider.id, "mock");
}

#[test]
fn call_jev_without_key_exits_1() {
    let dir = isolated_dir();
    let assert = tdmm_in(dir.path())
        .args(["call", "--provider", "jev"])
        .write_stdin(VALID_REQUEST)
        .assert()
        .failure();

    let output = assert.get_output();
    assert_eq!(output.status.code(), Some(1), "config failure -> exit 1");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("TDM_JEV_API_KEY"),
        "stderr explains how to fix the missing key"
    );
}

// ---------------------------------------------------------------------------
// 5. call through the runtime: caching across invocations
// ---------------------------------------------------------------------------

#[test]
fn call_through_runtime_caches_identical_results() {
    let dir = isolated_dir();

    let first = stdout_text(
        &tdmm_in(dir.path())
            .arg("call")
            .write_stdin(VALID_REQUEST)
            .assert()
            .success(),
    );
    let second = stdout_text(
        &tdmm_in(dir.path())
            .arg("call")
            .write_stdin(VALID_REQUEST)
            .assert()
            .success(),
    );

    let first: serde_json::Value = serde_json::from_str(&first).expect("valid result JSON");
    let second: serde_json::Value = serde_json::from_str(&second).expect("valid result JSON");
    assert_eq!(first, second, "cached second response must be identical");

    // The second call is audited with cached = true (newest row first).
    let assert = tdmm_in(dir.path())
        .args(["logs", "--json"])
        .assert()
        .success();
    let rows: Vec<serde_json::Value> =
        serde_json::from_str(&stdout_text(&assert)).expect("logs --json array");
    assert_eq!(rows.len(), 2, "two audited calls");
    assert_eq!(rows[0]["cached"], true, "second call was a cache hit");
    assert_eq!(rows[1]["cached"], false, "first call was a miss");
    assert_eq!(rows[0]["provider"], "mock");
}

// ---------------------------------------------------------------------------
// 6. use — default provider switch (comments preserved)
// ---------------------------------------------------------------------------

#[test]
fn use_switches_default_and_preserves_comments() {
    let dir = tempfile::tempdir().unwrap();
    let audit = dir.path().join("audit.db");
    fs::write(
        dir.path().join("config.toml"),
        format!(
            "# top comment stays\nversion = 1\n\n[provider.mock]\ntype = \"mock\"\n\n\
             [provider.jev]\ntype = \"jev\"\ntimeout_ms = 10000\n\n\
             [defaults]\nprovider = \"jev\"    # switch me\nconfidence_floor = 0.55\n\n\
             [audit]\npath = '{}'\n",
            audit.display()
        ),
    )
    .unwrap();
    let before = fs::read_to_string(dir.path().join("config.toml")).unwrap();

    let assert = tdmm_in(dir.path()).args(["use", "mock"]).assert().success();
    assert!(stdout_text(&assert).contains("mock"));

    let after = fs::read_to_string(dir.path().join("config.toml")).unwrap();
    assert!(after.contains("# top comment stays"), "header comment kept");
    assert!(after.contains("# switch me"), "inline comment kept");
    assert!(after.contains("provider = \"mock\""), "default rewritten");
    assert!(!after.contains("provider = \"jev\""), "old default gone");
    assert!(after.contains("[provider.jev]"), "provider table untouched");
    assert!(after.contains("timeout_ms = 10000"), "provider entry kept");
    assert!(after.contains("confidence_floor = 0.55"), "siblings kept");

    // Effective end to end: the new default routes `call` to mock.
    let result: DecisionResult = serde_json::from_str(&stdout_text(
        &tdmm_in(dir.path())
            .arg("call")
            .write_stdin(VALID_REQUEST)
            .assert()
            .success(),
    ))
    .unwrap();
    assert_eq!(result.provider.id, "mock");
    let _ = before; // (snapshot used implicitly above via assertions)
}

#[test]
fn use_rejects_unknown_provider() {
    let dir = tempfile::tempdir().unwrap();
    let audit = dir.path().join("audit.db");
    fs::write(
        dir.path().join("config.toml"),
        format!(
            "version = 1\n\n[provider.mock]\ntype = \"mock\"\n\n\
             [defaults]\nprovider = \"mock\"\n\n[audit]\npath = '{}'\n",
            audit.display()
        ),
    )
    .unwrap();
    let before = fs::read_to_string(dir.path().join("config.toml")).unwrap();

    let assert = tdmm_in(dir.path())
        .args(["use", "startlux"])
        .assert()
        .failure();
    assert_eq!(
        assert.get_output().status.code(),
        Some(1),
        "unknown provider -> exit 1"
    );
    let stderr = stderr_text(&assert);
    assert!(stderr.contains("not defined"), "stderr names the problem");

    // Untouched on refusal.
    let after = fs::read_to_string(dir.path().join("config.toml")).unwrap();
    assert_eq!(after, before);
}

#[test]
fn use_without_config_fails() {
    let dir = tempfile::tempdir().unwrap();
    let assert = tdmm_in(dir.path()).args(["use", "mock"]).assert().failure();
    let stderr = stderr_text(&assert);
    assert!(stderr.contains("tdmm init"), "stderr points at `tdmm init`");
}

// ---------------------------------------------------------------------------
// 7. keys — auth.toml management with redaction
// ---------------------------------------------------------------------------

#[test]
fn keys_set_list_rm_roundtrip_redacts() {
    let dir = tempfile::tempdir().unwrap();
    let key = "sk-live-0123456789abcdef";
    let redacted = "sk-l…";

    // Positional key: allowed, but must warn about shell history.
    let assert = tdmm_in(dir.path())
        .args(["keys", "set", "mock", key])
        .assert()
        .success();
    let stdout = stdout_text(&assert);
    let stderr = stderr_text(&assert);
    assert!(stdout.contains("mock") && stdout.contains("saved"));
    assert!(stdout.contains(redacted), "confirmation shows redacted key");
    assert!(
        !stdout.contains(key) && !stderr.contains(key),
        "full key never echoed"
    );
    assert!(
        stderr.contains("history"),
        "positional key warns about history leak"
    );

    // The secret lives in auth.toml only.
    let auth = fs::read_to_string(dir.path().join("auth.toml")).unwrap();
    assert!(auth.contains("[mock]"));
    assert!(auth.contains(key));

    // list: provider + redacted key only.
    let assert = tdmm_in(dir.path())
        .args(["keys", "list"])
        .assert()
        .success();
    let stdout = stdout_text(&assert);
    assert!(stdout.contains("mock") && stdout.contains(redacted));
    assert!(!stdout.contains(key), "list never prints the full key");

    // rm removes the table; a second rm fails.
    tdmm_in(dir.path())
        .args(["keys", "rm", "mock"])
        .assert()
        .success();
    let auth = fs::read_to_string(dir.path().join("auth.toml")).unwrap();
    assert!(!auth.contains("[mock]"));
    tdmm_in(dir.path())
        .args(["keys", "rm", "mock"])
        .assert()
        .failure();

    // list after rm: nothing stored.
    let assert = tdmm_in(dir.path())
        .args(["keys", "list"])
        .assert()
        .success();
    assert!(stdout_text(&assert).contains("no keys stored"));
}

#[test]
fn keys_set_via_stdin_no_echo() {
    let dir = tempfile::tempdir().unwrap();
    let key = "jev-secret-1234567890";
    let redacted = "jev-…";

    let assert = tdmm_in(dir.path())
        .args(["keys", "set", "jev", "--stdin"])
        .write_stdin(format!("{key}\n"))
        .assert()
        .success();

    let stdout = stdout_text(&assert);
    let stderr = stderr_text(&assert);
    assert!(stdout.contains(redacted), "confirmation shows redacted key");
    assert!(
        !stdout.contains(key) && !stderr.contains(key),
        "full key never echoed"
    );
    assert!(!stderr.contains("history"), "stdin path does not warn");

    let auth = fs::read_to_string(dir.path().join("auth.toml")).unwrap();
    assert!(auth.contains(key), "trailing newline trimmed, key stored");
    assert!(auth.contains("[jev]"));
}

// ---------------------------------------------------------------------------
// 8. doctor — health report
// ---------------------------------------------------------------------------

#[test]
fn doctor_mock_only_passes() {
    let dir = isolated_dir();

    tdmm_in(dir.path()).arg("doctor").assert().success();

    let assert = tdmm_in(dir.path())
        .args(["doctor", "--json"])
        .assert()
        .success();
    let report: serde_json::Value =
        serde_json::from_str(&stdout_text(&assert)).expect("doctor --json object");
    assert_eq!(report["defaultHealthy"], true);
    assert_eq!(report["defaultProvider"], "mock");
    assert_eq!(report["auditDb"]["writable"], true);
    let providers = report["providers"].as_array().expect("providers array");
    assert_eq!(providers.len(), 1);
    assert_eq!(providers[0]["name"], "mock");
    assert_eq!(providers[0]["health"]["ok"], true);
    assert_eq!(providers[0]["health"]["version"], "mock-1");
}

#[test]
fn doctor_keyless_jev_default_is_unhealthy() {
    let dir = tempfile::tempdir().unwrap();
    let audit = dir.path().join("audit.db");
    fs::write(
        dir.path().join("config.toml"),
        format!(
            "version = 1\n\n[provider.jev]\ntype = \"jev\"\n\n\
             [provider.mock]\ntype = \"mock\"\n\n\
             [defaults]\nprovider = \"jev\"\n\n[audit]\npath = '{}'\n",
            audit.display()
        ),
    )
    .unwrap();

    let assert = tdmm_in(dir.path()).arg("doctor").assert().failure();
    assert_eq!(
        assert.get_output().status.code(),
        Some(1),
        "unhealthy default provider -> exit 1"
    );
    let stderr = stderr_text(&assert);
    assert!(
        stderr.contains("no API key") && stderr.contains("TDM_JEV_API_KEY"),
        "exclusion warning names the fix"
    );
    let stdout = stdout_text(&assert);
    assert!(
        stdout.contains("UNHEALTHY"),
        "human report shows the verdict"
    );
}

#[test]
fn doctor_keyless_jev_excluded_default_mock_still_ok() {
    let dir = tempfile::tempdir().unwrap();
    let audit = dir.path().join("audit.db");
    fs::write(
        dir.path().join("config.toml"),
        format!(
            "version = 1\n\n[provider.jev]\ntype = \"jev\"\n\n\
             [provider.mock]\ntype = \"mock\"\n\n\
             [defaults]\nprovider = \"mock\"\n\n[audit]\npath = '{}'\n",
            audit.display()
        ),
    )
    .unwrap();

    let assert = tdmm_in(dir.path()).arg("doctor").assert().success();
    assert_eq!(
        assert.get_output().status.code(),
        Some(0),
        "exit follows the DEFAULT provider's health"
    );
    assert!(
        stderr_text(&assert).contains("no API key"),
        "jev is still excluded with a warning"
    );
    let stdout = stdout_text(&assert);
    assert!(stdout.contains("mock") && stdout.contains("healthy"));
}

// ---------------------------------------------------------------------------
// 9. logs / stats — audit queries
// ---------------------------------------------------------------------------

#[test]
fn logs_and_stats_reflect_call_history() {
    let dir = isolated_dir();
    for _ in 0..2 {
        tdmm_in(dir.path())
            .arg("call")
            .write_stdin(VALID_REQUEST)
            .assert()
            .success();
    }

    // logs --json: two rows, newest (the cache hit) first.
    let assert = tdmm_in(dir.path())
        .args(["logs", "--json"])
        .assert()
        .success();
    let rows: Vec<serde_json::Value> =
        serde_json::from_str(&stdout_text(&assert)).expect("logs --json array");
    assert_eq!(rows.len(), 2, "both calls audited");
    assert!(
        rows.iter().all(|row| row["harness"] == "tdmm"),
        "rows tagged with the tdmm harness"
    );
    assert_eq!(
        rows.iter().filter(|row| row["cached"] == true).count(),
        1,
        "exactly one cache hit"
    );

    // Table form renders the documented columns.
    let assert = tdmm_in(dir.path()).arg("logs").assert().success();
    let stdout = stdout_text(&assert);
    for column in [
        "harness",
        "session",
        "provider",
        "model",
        "cached",
        "latency_ms",
        "tokens",
    ] {
        assert!(stdout.contains(column), "table header has {column}");
    }

    // Filters and limit.
    let assert = tdmm_in(dir.path())
        .args(["logs", "--json", "--harness", "nope"])
        .assert()
        .success();
    let rows: Vec<serde_json::Value> = serde_json::from_str(&stdout_text(&assert)).unwrap();
    assert!(rows.is_empty(), "harness filter excludes everything");

    let assert = tdmm_in(dir.path())
        .args(["logs", "--json", "--limit", "1"])
        .assert()
        .success();
    let rows: Vec<serde_json::Value> = serde_json::from_str(&stdout_text(&assert)).unwrap();
    assert_eq!(rows.len(), 1, "--limit caps the rows");

    // stats --json: one aggregate row with both calls and the cache hit.
    let assert = tdmm_in(dir.path())
        .args(["stats", "--json"])
        .assert()
        .success();
    let stats: Vec<serde_json::Value> =
        serde_json::from_str(&stdout_text(&assert)).expect("stats --json array");
    assert_eq!(stats.len(), 1);
    assert_eq!(stats[0]["provider"], "mock");
    assert_eq!(stats[0]["harness"], "tdmm");
    assert_eq!(stats[0]["calls"], 2);
    assert!(
        stats[0]["cacheHits"].as_u64().unwrap() >= 1,
        "cache_hits >= 1"
    );

    // stats table form.
    let assert = tdmm_in(dir.path()).arg("stats").assert().success();
    let stdout = stdout_text(&assert);
    for column in [
        "provider",
        "harness",
        "calls",
        "cache_hits",
        "avg_latency_ms",
    ] {
        assert!(stdout.contains(column), "stats table header has {column}");
    }
}

// ---------------------------------------------------------------------------
// 10. config path / validate
// ---------------------------------------------------------------------------

#[test]
fn config_path_prints_resolved_locations() {
    let dir = isolated_dir();
    let assert = tdmm_in(dir.path())
        .args(["config", "path"])
        .assert()
        .success();
    let stdout = stdout_text(&assert);
    assert!(stdout.contains("config.toml"));
    assert!(stdout.contains("auth.toml"));
    assert!(stdout.contains("audit.db"));
    assert!(
        stdout.contains(dir.path().to_string_lossy().as_ref()),
        "paths resolve under TDM_CONFIG_DIR"
    );
}

#[test]
fn config_validate_reports_ok_and_errors() {
    let dir = isolated_dir();
    let assert = tdmm_in(dir.path())
        .args(["config", "validate"])
        .assert()
        .success();
    let stdout = stdout_text(&assert);
    assert!(stdout.contains("config: ok"), "valid config reported ok");
    assert!(
        stdout.contains("auth: missing"),
        "missing auth is not an error"
    );

    // Broken config.toml -> exit 1 with a parse report.
    fs::write(dir.path().join("config.toml"), "[[definitely not toml").unwrap();
    let assert = tdmm_in(dir.path())
        .args(["config", "validate"])
        .assert()
        .failure();
    assert_eq!(assert.get_output().status.code(), Some(1));
    assert!(stdout_text(&assert).contains("config: ERROR"));

    // Restore config, break auth.toml -> still exit 1.
    write_mock_config(dir.path());
    fs::write(dir.path().join("auth.toml"), "[jev\nbroken").unwrap();
    let assert = tdmm_in(dir.path())
        .args(["config", "validate"])
        .assert()
        .failure();
    assert_eq!(assert.get_output().status.code(), Some(1));
    assert!(stdout_text(&assert).contains("auth: ERROR"));
}
