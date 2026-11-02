//! COP-86 (H-078): `--version` build facts, shell completions, the man page,
//! and bare `coppa` as a usage error.

mod common;

use common::coppa_command;
use std::process::Output;

fn run(args: &[&str]) -> Output {
    coppa_command().args(args).output().expect("run coppa")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn version_flag_long_and_short() {
    let long = run(&["--version"]);
    assert!(long.status.success(), "{}", stderr(&long));
    let text = stdout(&long);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 4, "{text}");
    assert_eq!(lines[0], format!("coppa {}", env!("CARGO_PKG_VERSION")));
    assert!(lines[1].starts_with("commit: "), "{text}");
    assert!(lines[2].starts_with("features: "), "{text}");
    assert_eq!(lines[3], "wire format: 1");

    let short = run(&["-V"]);
    assert!(short.status.success());
    assert_eq!(
        stdout(&short).lines().collect::<Vec<_>>(),
        [format!("coppa {}", env!("CARGO_PKG_VERSION"))]
    );
}

#[test]
fn bare_coppa_prints_help_and_exits_2() {
    let output = run(&[]);
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(stdout(&output), "");
    assert!(
        stderr(&output).contains("Usage: coppa"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn completions_for_each_shell() {
    let expected = [
        ("bash", "_coppa()"),
        ("zsh", "#compdef coppa"),
        ("fish", "complete -c coppa"),
        ("powershell", "Register-ArgumentCompleter"),
        ("elvish", "coppa"),
    ];
    for (shell, needle) in expected {
        let output = run(&["completions", shell]);
        assert!(output.status.success(), "{shell}: {}", stderr(&output));
        let script = stdout(&output);
        assert!(script.contains(needle), "{shell} lacks {needle:?}");
        if shell == "zsh" {
            assert!(script.starts_with("#compdef coppa"), "{shell}");
        }
    }
    let bash = stdout(&run(&["completions", "bash"]));
    let top = bash
        .lines()
        .find(|line| line.contains("opts=\"-v"))
        .expect("top-level opts line");
    for name in [
        "tx",
        "rx",
        "loopback",
        "tune",
        "devices",
        "doctor",
        "profiles",
        "completions",
        "man",
    ] {
        assert!(
            top.split(['"', ' ']).any(|word| word == name),
            "bash completion lacks {name}: {top}"
        );
    }
    for hidden in ["listen", "config"] {
        assert!(
            !top.split(['"', ' ']).any(|word| word == hidden),
            "bash completion offers hidden {hidden}: {top}"
        );
    }
}

#[test]
fn completions_rejects_unknown_shell() {
    let output = run(&["completions", "nushell"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(
        stderr(&output).contains("possible values"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn man_page_is_roff() {
    let output = run(&["man"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let page = stdout(&output);
    assert!(page.contains(".TH coppa 1"), "{page}");
    assert!(page.contains("tune"), "{page}");
    assert!(page.contains("completions"), "{page}");
}

#[cfg(unix)]
#[test]
fn completions_survive_a_closed_pipe() {
    use std::process::Stdio;
    let mut child = coppa_command()
        .args(["completions", "bash"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn coppa");
    drop(child.stdout.take());
    let output = child.wait_with_output().expect("wait for coppa");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(!stderr(&output).contains("panicked"), "{}", stderr(&output));
}
