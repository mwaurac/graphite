use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static COUNTER: AtomicU64 = AtomicU64::new(0);

struct TempDirectory(PathBuf);

impl TempDirectory {
    fn new() -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "graphite-cli-test-{}-{nanos}-{id}",
            std::process::id()
        ));
        fs::create_dir_all(&path).unwrap();
        TempDirectory(path)
    }
}

impl Drop for TempDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

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
fn persistent_database_survives_between_invocations() {
    let directory = TempDirectory::new();
    let path = directory.0.to_str().unwrap();

    let create = graphite(&[
        "--db",
        path,
        "-e",
        r#"CREATE (:Person {name: "Alice"})-[:KNOWS]->(:Person {name: "Bob"})"#,
    ]);
    assert!(
        create.status.success(),
        "{}",
        String::from_utf8_lossy(&create.stderr)
    );

    let read = graphite(&[
        "--db",
        path,
        "-e",
        r#"MATCH (p:Person) RETURN p.name AS name ORDER BY name"#,
    ]);
    assert!(
        read.status.success(),
        "{}",
        String::from_utf8_lossy(&read.stderr)
    );
    let stdout = String::from_utf8(read.stdout).unwrap();
    assert!(stdout.contains("Alice"));
    assert!(stdout.contains("Bob"));
    assert!(directory.0.join("snapshot.gql").exists());
}

#[test]
fn a_failed_cli_query_does_not_persist_partial_writes() {
    let directory = TempDirectory::new();
    let path = directory.0.to_str().unwrap();

    let create = graphite(&["--db", path, "-e", "CREATE (:Kept)"]);
    assert!(create.status.success());

    let failed = graphite(&[
        "--db",
        path,
        "-e",
        r#"CREATE (a:Person {name: "A"}), (a:Person {name: "B"})"#,
    ]);
    assert!(!failed.status.success());

    let read = graphite(&["--db", path, "-e", "MATCH (n) RETURN count(*) AS count"]);
    assert!(read.status.success());
    let stdout = String::from_utf8(read.stdout).unwrap();
    assert!(stdout.contains('1'), "{stdout}");
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
