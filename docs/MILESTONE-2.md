# Graphite — Milestone 2a: Atomicity & Durability

**Version:** 0.1.0  
**Status:** Complete for the single-writer, single-process slice  
**Scope:** Make acknowledged writes survive a crash, make scripts atomic, and add
an explicit transaction API. Concurrency, isolation levels, and a query planner
are still out of scope.

---

## 1. What changed

Milestone 1 was an in-memory database with an explicit snapshot command. A crash
lost everything since the last `:save`, and a script that failed halfway could
leave a partial graph.

Milestone 2a adds:

1. **Atomic scripts.** `Database::execute` treats every call as one transaction.
   If any statement fails, no statement in that call is applied.
2. **Explicit transactions.** `Database::begin()` returns a `Transaction` with
   `execute`, `commit`, and `rollback`. A failed transaction is aborted and cannot
   be committed.
3. **Durable commits.** `Database::open(dir)` enables a write-ahead log. A commit
   returns only after its WAL record has been written and `fsync`ed.
4. **Crash recovery.** On startup Graphite loads the snapshot, replays valid WAL
   records with a sequence greater than the snapshot's, and checkpoints.
5. **Torn-tail tolerance.** A partial or corrupt final WAL record is ignored,
   treating it as a transaction that never acknowledged a commit.
6. **Atomic snapshots.** Checkpoints write `snapshot.gql.tmp`, `fsync` it, rename
   it over `snapshot.gql`, and `fsync` the directory. The WAL is truncated only
   after the new snapshot is durable.
7. **Persistent CLI mode.** `graphite --db <dir>` opens or creates a database
   directory. `:checkpoint` forces a snapshot and WAL truncation.

The language, graph model, and query results are unchanged.

---

## 2. Transaction semantics

```rust
let mut db = Database::open("data/social")?;

let mut tx = db.begin();
tx.execute(r#"CREATE (:Person {name: "Alice"})"#)?;
tx.execute(r#"CREATE (:Person {name: "Bob"})"#)?;
tx.commit()?;
```

- A transaction is staged on a **copy of the graph**, created lazily on the first
  write. Read-only transactions and read-only queries do not copy the graph.
- `commit` appends one WAL record for the transaction and then installs the staged
  graph in memory.
- Dropping a transaction without committing rolls it back. `rollback` is an
  explicit no-op that consumes the transaction for readability.
- Any execution error aborts the transaction. This matches the common SQL
  behaviour: a failed transaction must be rolled back rather than partially
  committed.
- `Database::execute` is a convenience wrapper that begins, executes, and commits
  one transaction. Multiple `;`-separated statements in one call are therefore
  all-or-nothing.
- There is one writer at a time because the API requires `&mut Database`.
  Transactions do not provide isolation between processes or threads.

---

## 3. On-disk layout

A persistent database is a directory:

```text
data/social/
  snapshot.gql      graph as of a sequence number, written atomically
  wal.log           append-only records for commits after the snapshot
```

`snapshot.gql` begins with a comment that stores the sequence:

```cypher
// graphite-snapshot seq=42
// Graphite snapshot
CREATE (:Person {name: "Alice"});
...
```

The snapshot body is still a normal Graphite script, so it remains inspectable
and importable. Older snapshots without the sequence comment are treated as
sequence `0`.

### WAL record format

Each record is:

```text
[sequence: u64 little-endian]
[payload length: u64 little-endian]
[crc32(sequence || payload): u32 little-endian]
[payload: UTF-8 Graphite statements]
```

The file starts with `GRAPHITE-WAL-1\n`. A transaction's sources are joined with
`\n;\n` into one payload, so replay applies the whole transaction or nothing.

Recovery reads records from the start and stops at the first record that is
truncated, too large, or fails its checksum. A record at or below the snapshot
sequence is skipped. After replay, Graphite writes a fresh snapshot and truncates
the WAL.

### Durability ordering

Commit:

1. Append the WAL record.
2. `fsync` the WAL.
3. Install the staged graph in memory.
4. Return success.

Checkpoint:

1. Write `snapshot.gql.tmp` with the new sequence.
2. `fsync` the file.
3. Rename over `snapshot.gql`.
4. `fsync` the directory.
5. Truncate the WAL and write its header.
6. `fsync` the WAL.

The ordering makes the interesting crash windows safe:

| Crash point | Recovery result |
| --- | --- |
| During WAL append | Partial record is ignored; transaction is lost as unacknowledged |
| After WAL `fsync`, before in-memory install | Record is replayed; transaction survives |
| During snapshot write | Old snapshot remains; WAL replays |
| After snapshot rename, before WAL truncation | WAL records at or below snapshot sequence are skipped |
| After WAL truncation | New snapshot is already durable |

---

## 4. Tests added

`tests/durability.rs` covers:

- A statement that fails midway leaves the graph byte-for-byte unchanged.
- Explicit `commit` and `rollback`.
- An aborted transaction cannot commit.
- Read-only transactions do not advance the sequence.
- Committed nodes and relationships survive reopening.
- Multiple transactions replay in order.
- Snapshot plus later WAL records recover together.
- Writing after reopening a checkpointed database appends to the WAL correctly.
- A torn WAL tail is ignored.
- A corrupt WAL header is rejected.
- Reopening compacts the WAL and preserves the snapshot sequence.

`src/storage.rs` unit tests cover WAL round-trips, appending after reopening an
existing log, torn tails, header repair, checksum corruption, snapshot sequence
parsing, and checkpoint truncation. `tests/cli.rs` adds persistent CLI round-trips
and checks that a failed CLI query does not persist partial writes.

---

## 5. Known limitations

These are deliberate cuts for this slice, not accidental omissions:

### Storage and performance

- **Whole-graph copy per write transaction.** The first write clones the graph.
  This is correct but O(graph size) per transaction and unsuitable for large
  databases. The next step is an undo log or copy-on-write node/edge storage.
- **Full snapshot checkpoints.** Every checkpoint serialises the entire graph.
  A production design needs incremental checkpoints or a log-structured store.
- **The WAL grows until an explicit checkpoint or reopen.** There is no background
  checkpoint policy, group commit, or WAL size limit.
- **No file locking.** Two processes can open the same directory and corrupt each
  other's WAL. A lock file with advisory locking is required.
- **No CRC protection for snapshots.** Snapshot corruption is detected only when
  the script fails to parse or execute.
- **No format version migration.** Snapshot and WAL formats are versioned by
  convention only.
- **`fsync` of a directory is Unix-oriented.** The implementation targets macOS
  and Linux.

### Transactions and concurrency

- **Single writer, no readers during a write.** `&mut Database` prevents sharing
  within Rust, but there is no multi-process protection or MVCC.
- **No isolation levels.** A transaction sees its own staged writes; other
  transactions cannot run concurrently because of the mutable borrow.
- **Abort-on-first-error.** There are no savepoints.
- **No durability level configuration.** Every commit pays for `fsync`.
- **No transaction timeout or deadlock handling.** Deadlocks cannot occur without
  concurrency, but the API has no policy for future locking.
- **Commit errors can be ambiguous.** If a WAL `fsync` fails after bytes reached
  the file, `commit` returns an error but recovery may still apply the
  transaction. The database is not poisoned after an I/O error; a production
  system should fail closed and require an explicit reopen.

### Correctness and semantics

- **No semantic-analysis pass.** Some errors are discovered after earlier
  statements in the same transaction have mutated the staged copy. Atomicity
  prevents those changes from being committed, but a pre-execution validation
  pass would be cheaper and clearer.
- **No constraints or uniqueness.** Duplicate logical entities are still allowed.
- **No parameters.** Queries are re-parsed and re-logged as source text.
- **Replay depends on deterministic query execution.** IDs and iteration order are
  deterministic today; future nondeterministic functions would need a physical
  redo log instead of a logical one.

---

## 6. Recommended next steps

In priority order:

1. **Process locking** so a database directory has a single writer across
   processes. This is the most serious remaining safety gap.
2. **Undo-log transactions** to remove the whole-graph clone per write.
3. **Savepoints and statement-level error recovery** for long transactions.
4. **Semantic analysis before mutation** to reject invalid label/property updates
   and bound-variable reuse up front.
5. **Constraints and unique indexes**, especially uniqueness on a chosen identity
   property.
6. **Checkpoint policy** based on WAL bytes or commit count, with an online
   checkpoint that does not stall large writes.
7. **Parameterised queries**, then property indexes and `EXPLAIN`.
8. **Snapshot checksums and format versioning**, then migration tests.
9. **Multi-process/server mode and MVCC** only after the single-node storage
   layer is efficient.

---

## 7. How this changes the Milestone 1 assessment

The Milestone 1 gap analysis listed statement atomicity, WAL durability, crash
recovery, and a transaction API as the top four recommendations. This slice
implements the single-process versions of all four. The remaining top gap is
**process safety**: without locking, the storage engine is still not safe to share
between programs. After that, performance work (copy-free transactions and
incremental checkpoints) matters more than new syntax.
