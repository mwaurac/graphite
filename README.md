# Graphite

A small property graph database with its own query language, written in Rust with
no external dependencies.

This repository currently contains **Milestone 2a**: a single-process graph engine
with a hand-written lexer, parser, and executor, plus atomic transactions,
write-ahead-log durability, and crash recovery. It is small enough to read
end-to-end while still supporting real graph traversal queries.

For a detailed status report, a gap analysis against production graph databases,
and a prioritized roadmap, see
[`docs/MILESTONE-1.md`](docs/MILESTONE-1.md). For the transaction and durability
design added in Milestone 2a, see
[`docs/MILESTONE-2.md`](docs/MILESTONE-2.md).

```text
$ cargo run
Graphite 0.1.0 — a small graph database
Type `:help` for help, `:demo` for sample data, `:quit` to exit.

graphite> :demo
Loaded demo graph: 6 nodes, 7 relationships.

graphite> MATCH (a:Person)-[:KNOWS]->(b:Person)
       ... RETURN a.name AS from, b.name AS to ORDER BY from;
+---------+---------+
| from    | to      |
+---------+---------+
| "Alice" | "Bob"   |
| "Bob"   | "Carol" |
| "Carol" | "Dave"  |
+---------+---------+
3 rows
```

## Quick start

Requires Rust 1.75 or newer.

```bash
# Interactive shell
cargo run

# Run a single query
cargo run -- -e 'CREATE (n:Person {name: "Alice"}) RETURN n'

# Run a script
cargo run -- examples/social.gql

# Open a persistent database (creates the directory if needed)
cargo run -- --db data/social
cargo run -- --db data/social -e 'MATCH (n) RETURN count(*)'

# Run the test suite
cargo test
```

Inside the shell:

| Command | Description |
| --- | --- |
| `:help` | Show help and query examples |
| `:demo` | Reset the database and load a sample social graph |
| `:stats` | Show counts, sequence number, and storage directory |
| `:checkpoint` | Snapshot a persistent database and truncate its WAL |
| `:save <file>` | Save the graph as a Graphite script |
| `:load <file>` | Replace the graph with a saved script |
| `:clear` | Remove all data |
| `:quit` | Exit |

Statements run when they parse completely, when they end with `;`, or when you
press Enter on an empty line.

## The query language

Graphite's language is Cypher-flavoured. It is line-oriented and case-insensitive
for keywords. There is exactly one statement kind: `CREATE` or `MATCH` followed
by an action.

### Creating data

```cypher
CREATE (a:Person {name: "Alice", age: 30, city: "Nairobi"});
CREATE (a:Person {name: "Alice"})-[:KNOWS {since: 2019}]->(b:Person {name: "Bob"});

// One statement can create several patterns that share variables.
CREATE
  (a:Person {name: "Alice"}),
  (b:Person {name: "Bob"}),
  (a)-[:KNOWS]->(b)
RETURN a.name, b.name;
```

Nodes are written `(variable:Label {key: value, ...})`; relationships are written
`-[variable:TYPE {key: value}]->`, `<-[...]-`, or `-[...]-`. Relationship types are
required in `CREATE`. Node and relationship variables are optional.

### Reading data

```cypher
MATCH (p:Person) RETURN p.name, p.age;

MATCH (p:Person)
WHERE p.age >= 30 AND p.city = "Nairobi"
RETURN p.name AS name, p.age AS age
ORDER BY p.age DESC
LIMIT 10;

// Traversal, with an optional relationship variable and property filter
MATCH (a:Person)-[r:KNOWS {since: 2019}]->(b:Person)
WHERE a.name = "Alice"
RETURN a.name, b.name, r.since;

// Multi-hop paths and reverse/undirected edges
MATCH (a:Person)-[:KNOWS]->(b:Person)-[:KNOWS]->(c:Person)
RETURN a.name, b.name, c.name;

MATCH (a:Person)<-[:KNOWS]-(b:Person) RETURN a.name, b.name;
MATCH (a:Person)-[:KNOWS]-(b:Person) RETURN a.name, b.name;

// Aggregation; non-aggregate RETURN items are the grouping keys
MATCH (p:Person)
RETURN p.city AS city, count(*) AS people, avg(p.age) AS average_age
ORDER BY people DESC;

// DISTINCT, SKIP, LIMIT, and SELECT *
MATCH (p:Person) RETURN DISTINCT p.city ORDER BY p.city SKIP 1 LIMIT 5;
MATCH (a:Person)-[r:KNOWS]->(b:Person) RETURN *;
```

Returned nodes and relationships render as signatures:

```text
(:Person {age: 30, city: "Nairobi", name: "Alice"})
[:KNOWS {since: 2019}]
```

### Updating and deleting

```cypher
// SET accepts one or more property assignments and can return the result
MATCH (p:Person {name: "Alice"})
SET p.age = p.age + 1, p.active = true
RETURN p.age, p.active;

// Delete a relationship
MATCH (a:Person {name: "Alice"})-[r:KNOWS]->(b:Person) DELETE r;

// A node with relationships requires DETACH DELETE
MATCH (p:Person {name: "Alice"}) DETACH DELETE p;

MATCH (p:Person) DETACH DELETE p RETURN p.name;
```

### Expressions

| Category | Supported |
| --- | --- |
| Literals | `null`, booleans, integers, floats, strings, lists |
| Operators | `+ - * / %`, `= <> != < <= > >=`, `AND OR NOT`, `IS [NOT] NULL` |
| Paths | property access `n.key`, `r.key` |
| Functions | `id`, `labels`, `type`, `keys`, `coalesce`, `lower`, `upper` |
| Aggregates | `count(*)`, `count`, `sum`, `avg`, `min`, `max` |
| Clauses | `WHERE`, `RETURN [DISTINCT]`, `ORDER BY`, `SKIP`, `LIMIT`, `AS` |

### Semantics worth knowing

- Properties live on nodes and relationships; missing properties evaluate to
  `null`.
- `WHERE` only passes rows whose expression evaluates to `true`; `null` fails.
- Relationship types are single, required for creation, and matched exactly.
- Patterns are expanded by backtracking from the label and adjacency indexes.
- Node and relationship IDs are 1-based and stable for the life of the database.
- A relationship cannot be bound to two variables in the same pattern.
- Property matching in patterns uses strict equality (`null` matches `null`).
- `CREATE` cannot attach labels/properties to an already-bound variable; refer to
  it as `(existing)` instead.

## Durability and transactions

Graphite has two modes.

**In-memory** is the default. It is useful for exploration and tests, and can be
saved explicitly as a Graphite script:

```text
graphite> :demo
graphite> :save social.gql
Saved 6 nodes and 7 relationships to `social.gql`.
graphite> :load social.gql
Loaded 6 nodes and 7 relationships from `social.gql`.
```

**Persistent** mode is enabled with `--db <directory>` or `Database::open`:

```bash
cargo run -- --db data/social
```

The directory contains `snapshot.gql` and `wal.log`. Each committed transaction
is appended to the WAL and `fsync`ed before the commit returns. On startup the
snapshot is loaded, valid WAL records are replayed, and the database checkpoints.
A torn final WAL record is ignored, so an interrupted write is treated as a
transaction that never committed.

Every `Database::execute` call is atomic: if any statement fails, none of that
call's changes are applied. For larger units of work, use an explicit transaction:

```rust
use graphite::Database;

let mut db = Database::open("data/social")?;

let mut tx = db.begin();
tx.execute(r#"CREATE (:Person {name: "Alice"})"#)?;
tx.execute(r#"CREATE (:Person {name: "Bob"})"#)?;
tx.commit()?; // one durable WAL record

let mut tx = db.begin();
tx.execute(r#"CREATE (:Person {name: "Discarded"})"#)?;
tx.rollback(); // nothing is written
```

A transaction that hits an execution error is aborted and cannot be committed.
Read-only transactions do not copy the graph; the first write stages a graph copy
for atomic commit. Only one writer can exist at a time, and there is no
multi-process locking yet. See [`docs/MILESTONE-2.md`](docs/MILESTONE-2.md) for the
on-disk format, crash windows, and current limitations.

The query language is also still an import/export format. A snapshot is a valid
Graphite script:

```rust
let snapshot = db.dump();                  // a valid Graphite script
let restored = Database::from_script(&snapshot)?;
```

## Library API

The engine is usable as a library:

```rust
use graphite::Database;

let mut db = Database::new();

db.execute(r#"CREATE (a:Person {name: "Alice"})-[:KNOWS]->(b:Person {name: "Bob"})"#)
    .unwrap();

let results = db
    .execute(
        r#"MATCH (a:Person)-[:KNOWS]->(b:Person)
           RETURN a.name AS from, b.name AS to"#,
    )
    .unwrap();

for row in &results[0].rows {
    println!("{} knows {}", row[0], row[1]);
}
```

Each call returns one `QueryResult` per statement, with `columns`, `rows`, and
mutation `stats`.

## Project layout

```text
src/
  lexer.rs      tokens, source positions, comments
  ast.rs        statements, patterns, expressions
  parser.rs     recursive-descent parser
  graph.rs      property graph, label index, adjacency index
  executor.rs   transactions, pattern matching, mutation, aggregation
  storage.rs    WAL, atomic snapshots, crash recovery
  serial.rs     graph snapshots as Graphite scripts
  value.rs      values and their formatting/ordering
  error.rs      positioned errors
  main.rs       CLI, REPL, table rendering
tests/query.rs       end-to-end language tests
tests/durability.rs  transaction and crash-recovery tests
examples/            sample script
```

## Current scope

Implemented:

- Labeled nodes, typed directed relationships, properties, and indexes
- `CREATE`, `MATCH`, `WHERE`, `SET`, `DELETE`, `DETACH DELETE`, `RETURN`
- Multi-hop, reverse, and undirected traversal
- Filtering, property maps, aggregates, grouping, `DISTINCT`, `ORDER BY`,
  `SKIP`, `LIMIT`
- REPL with multiline input, scripts, single-query execution, and library API
- Atomic scripts, explicit commit/rollback transactions, and aborted-transaction
  protection
- A checksummed write-ahead log, atomic snapshot checkpoints, and torn-tail crash
  recovery
- Persistent database directories and snapshot save/load using the query language
  as the file format
- Positioned syntax errors and end-to-end tests, including recovery tests

Deliberately not in this milestone:

- Multi-process file locking, concurrency, MVCC, or configurable isolation
- Variable-length paths, `MERGE`, `OPTIONAL MATCH`, `UNWIND`, parameters
- Multiple relationship types and richer expressions/functions
- Constraints and unique indexes
- A cost-based planner and an efficient on-disk storage engine

The transaction layer currently stages a whole-graph copy on the first write, and
checkpoints serialise the entire graph. Those are the main performance limits to
address next, along with process locking. See
[`docs/MILESTONE-2.md`](docs/MILESTONE-2.md).

## License

MIT
