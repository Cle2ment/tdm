//! End-to-end tests for the `tdmm` binary: clap parsing shapes, `init`
//! skeleton bootstrap (idempotency / --force), and `call` through the mock
//! provider (exit-code contract, default provider selection).

use std::fs;
use std::path::PathBuf;

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
        .env_remove("TYPESAFE_API_KEY");
    cmd
}

fn stdout_text(assert: &assert_cmd::assert::Assert) -> String {
    String::from_utf8(assert.get_output().stdout.clone()).expect("stdout must be utf-8")
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
fn rejects_invalid_cli_shapes() {
    // No subcommand.
    assert!(Cli::try_parse_from(["tdmm"]).is_err());
    // Unknown subcommand.
    assert!(Cli::try_parse_from(["tdmm", "doctor"]).is_err());
    // Unknown provider value.
    assert!(Cli::try_parse_from(["tdmm", "call", "--provider", "startlux"]).is_err());
    // Unknown flag.
    assert!(Cli::try_parse_from(["tdmm", "init", "--clobber"]).is_err());
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
// 3. call end-to-end (mock)
// ---------------------------------------------------------------------------

#[test]
fn call_mock_via_stdin_answers_every_question() {
    let assert = tdmm()
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
    let dir = tempfile::tempdir().unwrap();
    let request_path = dir.path().join("request.json");
    fs::write(&request_path, VALID_REQUEST).unwrap();

    let assert = tdmm()
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
    let assert = tdmm()
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
    let assert = tdmm()
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
    let assert = tdmm()
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
    // Both key vars are stripped by the `tdmm()` helper; default must be mock
    // (offline success), never jev (which would fail on the missing key).
    let assert = tdmm()
        .arg("call")
        .write_stdin(VALID_REQUEST)
        .assert()
        .success();

    let result: DecisionResult =
        serde_json::from_str(&stdout_text(&assert)).expect("valid DecisionResult JSON");
    assert_eq!(result.provider.id, "mock", "no key env -> mock default");
}

#[test]
fn explicit_mock_beats_present_key_env() {
    let mut cmd = Command::cargo_bin("tdmm").unwrap();
    let assert = cmd
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
    let assert = tdmm()
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
