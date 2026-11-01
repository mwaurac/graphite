use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use graphite::{Database, Value};

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
            "graphite-durability-test-{}-{nanos}-{id}",
            std::process::id()
        ));
        fs::create_dir_all(&path).unwrap();
        TempDirectory(path)
    }

    fn wal(&self) -> PathBuf {
        self.0.join("wal.log")
    }

    fn snapshot(&self) -> PathBuf {
        self.0.join("snapshot.gql")
    }
}

impl Drop for TempDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn scalar(db: &mut Database, query: &str) -> Value {
    db.execute(query)
        .unwrap()
        .remove(0)
        .rows
        .remove(0)
        .remove(0)
}

#[test]
fn a_failed_statement_leaves_the_graph_unchanged() {
    let mut db = Database::new();
    db.execute(r#"CREATE (:Person {name: "base"})"#).unwrap();
    assert_eq!(db.graph().node_count(), 1);
    let before = db.dump();

    // The first pattern creates a node; the second fails because `a` is
    // already bound with a label and properties.
    let error = db
        .execute(r#"CREATE (a:Person {name: "A"}), (a:Person {name: "B"}), (:Person {name: "C"})"#)
        .unwrap_err();
    assert!(error.message.contains("already bound"));
    assert_eq!(db.graph().node_count(), 1);
    assert_eq!(db.dump(), before);
}

#[test]
fn transactions_commit_and_roll_back() {
    let mut db = Database::new();

    let mut transaction = db.begin();
    transaction
        .execute(r#"CREATE (:Person {name: "discarded"})"#)
        .unwrap();
    transaction.rollback();
    assert_eq!(db.graph().node_count(), 0);

    let mut transaction = db.begin();
    transaction
        .execute(r#"CREATE (:Person {name: "kept"})"#)
        .unwrap();
    transaction
        .execute(r#"MATCH (p:Person) SET p.seen = true"#)
        .unwrap();
    transaction.commit().unwrap();

    assert_eq!(db.graph().node_count(), 1);
    assert_eq!(
        scalar(&mut db, r#"MATCH (p:Person) RETURN p.seen"#),
        Value::Bool(true)
    );
}

#[test]
fn an_aborted_transaction_cannot_commit() {
    let mut db = Database::new();

    let mut transaction = db.begin();
    assert!(transaction
        .execute(r#"CREATE (a:Person {name: "A"}), (a:Person {name: "B"})"#)
        .is_err());
    assert!(transaction.is_failed());
    assert!(transaction.commit().is_err());
    assert_eq!(db.graph().node_count(), 0);
}

#[test]
fn a_parse_error_aborts_an_explicit_transaction() {
    let mut db = Database::new();
    let mut transaction = db.begin();

    assert!(transaction.execute("MATCH (n RETURN n").is_err());
    assert!(transaction.is_failed());
    assert!(transaction.commit().is_err());
    assert_eq!(db.graph().node_count(), 0);
}

#[test]
fn a_database_directory_is_locked_while_open() {
    let directory = TempDirectory::new();
    let db = Database::open(&directory.0).unwrap();

    let error = Database::open(&directory.0).unwrap_err();
    assert!(error.message.contains("already open"));

    drop(db);
    Database::open(&directory.0).unwrap();
}

#[test]
fn read_only_transactions_do_not_advance_the_sequence() {
    let mut db = Database::new();
    db.execute("CREATE (:Person)").unwrap();
    let sequence = db.last_sequence();

    let mut transaction = db.begin();
    transaction.execute("MATCH (p:Person) RETURN p").unwrap();
    transaction.commit().unwrap();

    assert_eq!(db.last_sequence(), sequence);
}

#[test]
fn committed_writes_survive_reopening() {
    let directory = TempDirectory::new();

    {
        let mut db = Database::open(&directory.0).unwrap();
        db.execute(r#"CREATE (:Person {name: "Alice"})-[:KNOWS]->(:Person {name: "Bob"})"#)
            .unwrap();
        assert_eq!(db.last_sequence(), 1);
    }

    {
        let mut db = Database::open(&directory.0).unwrap();
        assert_eq!(db.graph().node_count(), 2);
        assert_eq!(db.graph().edge_count(), 1);
        assert_eq!(db.last_sequence(), 1);
        assert_eq!(
            scalar(&mut db, r#"MATCH (p:Person {name: "Alice"}) RETURN p.name"#),
            Value::String("Alice".into())
        );
    }

    assert!(directory.snapshot().exists());
}

#[test]
fn multiple_transactions_replay_in_order() {
    let directory = TempDirectory::new();

    {
        let mut db = Database::open(&directory.0).unwrap();

        let mut transaction = db.begin();
        transaction
            .execute(r#"CREATE (:Counter {name: "c", value: 1})"#)
            .unwrap();
        transaction.commit().unwrap();

        db.execute(r#"MATCH (c:Counter) SET c.value = c.value + 1"#)
            .unwrap();
        db.execute(r#"MATCH (c:Counter) SET c.value = c.value + 1"#)
            .unwrap();
    }

    {
        let mut db = Database::open(&directory.0).unwrap();
        assert_eq!(
            scalar(&mut db, "MATCH (c:Counter) RETURN c.value"),
            Value::Int(3)
        );
        assert_eq!(db.graph().node_count(), 1);
    }
}

#[test]
fn checkpoint_and_new_writes_recover_correctly() {
    let directory = TempDirectory::new();

    {
        let mut db = Database::open(&directory.0).unwrap();
        db.execute(r#"CREATE (:A {name: "snapshotted"})"#).unwrap();
        db.checkpoint().unwrap();
        assert_eq!(db.last_sequence(), 1);

        db.execute(r#"CREATE (:B {name: "in-wal"})"#).unwrap();
        assert_eq!(db.last_sequence(), 2);
    }

    {
        let mut db = Database::open(&directory.0).unwrap();
        assert_eq!(db.graph().node_count(), 2);
        assert_eq!(db.last_sequence(), 2);
        assert_eq!(scalar(&mut db, "MATCH (n) RETURN count(*)"), Value::Int(2));
    }
}

#[test]
fn writes_after_reopening_a_checkpointed_database_are_recovered() {
    let directory = TempDirectory::new();

    {
        let mut db = Database::open(&directory.0).unwrap();
        db.execute(r#"CREATE (:A {name: "before"})"#).unwrap();
        db.checkpoint().unwrap();
    }

    {
        // The WAL is header-only here. Opening it must position appends at the
        // end rather than overwriting the header.
        let mut db = Database::open(&directory.0).unwrap();
        assert_eq!(db.graph().node_count(), 1);
        db.execute(r#"CREATE (:B {name: "after"})"#).unwrap();
    }

    {
        let mut db = Database::open(&directory.0).unwrap();
        assert_eq!(db.graph().node_count(), 2);
        assert_eq!(scalar(&mut db, "MATCH (n) RETURN count(*)"), Value::Int(2));
    }
}

#[test]
fn a_torn_wal_tail_is_ignored() {
    let directory = TempDirectory::new();

    {
        let mut db = Database::open(&directory.0).unwrap();
        db.execute(r#"CREATE (:A {name: "durable"})"#).unwrap();
    }

    let mut bytes = fs::read(directory.wal()).unwrap();
    bytes.extend_from_slice(&[0xff; 32]);
    fs::write(directory.wal(), bytes).unwrap();

    {
        let mut db = Database::open(&directory.0).unwrap();
        assert_eq!(db.graph().node_count(), 1);
        assert_eq!(
            scalar(&mut db, "MATCH (n:A) RETURN n.name"),
            Value::String("durable".into())
        );
    }
}

#[test]
fn a_corrupt_wal_header_is_rejected() {
    let directory = TempDirectory::new();

    {
        let _db = Database::open(&directory.0).unwrap();
    }
    fs::write(directory.wal(), b"not a graphite wal").unwrap();

    let error = Database::open(&directory.0).unwrap_err();
    assert!(error.message.contains("not a Graphite write-ahead log"));
}

#[test]
fn reopening_compacts_the_wal() {
    let directory = TempDirectory::new();

    {
        let mut db = Database::open(&directory.0).unwrap();
        db.execute("CREATE (:A)").unwrap();
        db.execute("CREATE (:B)").unwrap();
    }
    assert!(fs::metadata(directory.wal()).unwrap().len() > 0);

    {
        let db = Database::open(&directory.0).unwrap();
        assert_eq!(db.graph().node_count(), 2);
    }

    let snapshot = fs::read_to_string(directory.snapshot()).unwrap();
    assert!(snapshot.contains("// graphite-snapshot seq=2"));
    let wal = fs::read_to_string(directory.wal()).unwrap();
    assert_eq!(wal, "GRAPHITE-WAL-1\n");
}
