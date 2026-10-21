use std::process::Command;

fn graphite(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_graphite"))
        .args(args)
        .output()
        .expect("failed to run graphite")
}

#[test]
fn executes_a_query_from_the_command_line() {
    let output = graphite(&[
        "-e",
        r#"CREATE (n:Person {name: "Alice"}) RETURN n.name AS name"#,
    ]);

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("Alice"));
    assert!(stdout.contains("name"));
}

#[test]
fn reports_errors_with_a_failing_exit_code() {
    let output = graphite(&["-e", "MATCH (n) RETURN"]);

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("expected an expression"));
    assert!(stderr.contains("column"));
}

#[test]
fn prints_help_and_version() {
    let help = graphite(&["--help"]);
    assert!(help.status.success());
    assert!(String::from_utf8(help.stdout).unwrap().contains("USAGE"));

    let version = graphite(&["--version"]);
    assert!(version.status.success());
    assert!(String::from_utf8(version.stdout)
        .unwrap()
        .contains("graphite 0.1.0"));
}
