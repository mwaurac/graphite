# Graphite — Milestone 1 Status & Gap Analysis

**Version:** 0.1.0

**Status:** MVP complete, single-process, in-memory (historical snapshot)

**Purpose of this document:** record exactly what was built, what production graph
databases have that Graphite does not, and where to invest next.

> **Update:** Milestone 2a now adds atomic transactions, WAL durability, crash
> recovery, atomic snapshots, and directory locking. The gap analysis below is
> kept as the original assessment; for what has since been implemented and what
> remains, see [`MILESTONE-2.md`](MILESTONE-2.md).

---

## 1. Executive summary

Graphite is a small property-graph database written in Rust with **no external
crates**. Milestone 1 delivers a working end-to-end database:

- An in-memory property graph with labeled nodes, typed directed relationships,
  properties, label indexes, and adjacency indexes.
- A hand-written lexer, recursive-descent parser, and executor for a
  Cypher-flavoured language.
- Read and write queries, multi-hop traversal, filtering, aggregation, ordering,
  pagination, and deletes.
- A CLI/REPL, script execution, table output, and snapshot save/load.

It is deliberately a **learning-scale database**, not a production one. It has no
transactions, durability guarantees, concurrency control, network protocol, query
planner, or security model. Those are the main differences from Neo4j, Memgraph,
TigerGraph, JanusGraph, Dgraph, NebulaGraph, ArangoDB, Kùzu, FalkorDB, and Amazon
Neptune.

The most valuable next step is **not more query syntax** — it is making a single
node trustworthy: atomic statements, WAL-based durability, crash recovery, and
constraints. After that, language completeness and a cost-based planner matter
most.

---

## 2. What is implemented

### 2.1 Data model

| Feature | Status |
| --- | --- |
| Nodes with 1..n labels | ✅ |
| Directed relationships with one type | ✅ |
| Properties on nodes and relationships | ✅ |
| Property value types: null, bool, int, float, string, list, map | ✅ |
| Stable 1-based node and relationship IDs | ✅ |
| Node snapshots returned as values | ✅ |
| Relationship snapshots returned as values | ✅ |
| Multiple labels per node | ✅ |
| Self-loops | ✅ (supported by storage and matching) |
| Undirected / reverse traversal | ✅ (query-level direction) |
| Label index | ✅ (`HashMap<Label, BTreeSet<NodeId>>`) |
| Outgoing / incoming adjacency indexes | ✅ |
| Per-type adjacency indexes | ❌ (type is filtered after lookup) |

### 2.2 Query language

Implemented clauses and operators:

```cypher
CREATE (a:Person {name: "Alice", age: 30})-[:KNOWS {since: 2019}]->(b:Person {name: "Bob"});

MATCH (a:Person)-[:KNOWS]->(b:Person)-[:KNOWS]->(c:Person)
WHERE a.age >= 30 AND b.name <> "Bob"
RETURN a.name AS from, c.name AS reachable, count(*) AS paths
ORDER BY paths DESC, from
SKIP 1
LIMIT 10;

MATCH (p:Person {name: "Alice"})
SET p.age = p.age + 1, p.active = true
RETURN p.age;

MATCH (a:Person)-[r:KNOWS]->(b:Person) DELETE r;
MATCH (p:Person) DETACH DELETE p;
```

| Capability | Status |
| --- | --- |
| `CREATE` node / relationship patterns | ✅ |
| Multiple patterns per statement sharing variables | ✅ |
| `MATCH` fixed-length paths | ✅ |
| Multi-hop paths | ✅ |
| Left-to-right, right-to-left, undirected relationships | ✅ |
| Anonymous nodes / relationships | ✅ |
| Pattern labels and property maps | ✅ |
| Relationship variable and property filters | ✅ |
| `WHERE` with comparisons and boolean logic | ✅ |
| Three-valued `null` logic (`AND`, `OR`, `NOT`, `IS NULL`) | ✅ |
| `SET` one or more properties | ✅ |
| `DELETE` relationships / nodes | ✅ |
| `DETACH DELETE` | ✅ |
| `RETURN` expressions, aliases, `*` | ✅ |
| `DISTINCT` | ✅ |
| `ORDER BY` (aliases, source expressions, aggregate expressions) | ✅ |
| `SKIP`, `LIMIT` | ✅ |
| Aggregates `count`, `sum`, `avg`, `min`, `max` | ✅ |
| Grouping by non-aggregate return items | ✅ |
| Scalar functions `id`, `labels`, `type`, `keys`, `coalesce`, `lower`, `upper` | ✅ |
| Literals: int, float, string, bool, null, list, map | ✅ |
| Arithmetic `+ - * / %` with overflow checks | ✅ |
| String concatenation | ✅ |
| Multi-statement scripts (`;`) | ✅ |
| Positioned lexer/parser exceptions | ✅ |
| `RETURN` after `CREATE`, `SET`, and `DELETE` | ✅ |

### 2.3 Execution semantics

- Pattern matching uses label and adjacency indexes, with backtracking across
  chains and repeated variables.
- Unbound variables produce candidate sets; bound variables constrain later
  occurrences in the same `MATCH`.
- `WHERE` passes only rows that evaluate to `true`; `null` and non-booleans fail.
- `SET` updates the graph and refreshes the in-query binding snapshot.
- Deletes are collected from all matched rows, deduplicated, then applied.
  Deleting a connected node without `DETACH` is refused.
- Aggregation groups by all non-aggregate `RETURN` items.
- `ORDER BY`, `DISTINCT`, `SKIP`, and `LIMIT` run in projection order.
- Property keys are sorted in output and snapshots for deterministic results.

### 2.4 Interfaces

| Interface | Status |
| --- | --- |
| Interactive REPL with multiline input | ✅ |
| `:help`, `:demo`, `:stats`, `:clear`, `:quit` | ✅ |
| `:save <file>` / `:load <file>` | ✅ |
| Query via `-e/--execute` | ✅ |
| Script via `-f/--file` or positional path | ✅ |
| Piped stdin execution | ✅ |
| Table rendering | ✅ |
| Library API: `Database::execute`, `from_script`, `dump`, `graph` | ✅ |
| Machine-readable output (JSON/CSV) | ❌ |
| Network protocol / drivers | ❌ |

### 2.5 Persistence

Snapshots are written as Graphite scripts, so the query language is also the
import/export format:

```text
// Graphite snapshot
CREATE
  (n1:Person {age: 30, name: "Alice"}),
  (n2:Person {age: 27, name: "Bob"}),
  (n1)-[:KNOWS {since: 2019}]->(n2);
```

| Property | Status |
| --- | --- |
| Save full graph | ✅ |
| Load full graph | ✅ |
| Human-readable, grep-able format | ✅ |
| Round-trip of nested lists/maps | ✅ |
| Incremental writes | ❌ |
| Atomic file replacement / fsync | ❌ |
| Write-ahead log | ❌ |
| Crash recovery | ❌ |
| Backup / point-in-time recovery | ❌ |

### 2.6 Engineering

| Item | Status |
| --- | --- |
| Zero runtime dependencies | ✅ |
| Unit tests (lexer, parser, graph, executor, serialization) | ✅ 16 |
| Language integration tests | ✅ 19 |
| CLI integration tests | ✅ 3 |
| Doc test | ✅ 1 |
| **Total tests** | **39** |
| `cargo fmt` clean | ✅ |
| `cargo clippy -D warnings` clean | ✅ |
| GitHub Actions CI (fmt, clippy, test) | ✅ |
| Fuzzing / property-based tests | ❌ |
| Benchmarks | ❌ |
| Logging / tracing | ❌ |
| Metrics | ❌ |

Rough size: ~4,000 lines of Rust plus ~260 lines of README and tests.

---

## 3. Architecture

```text
                  ┌──────────────┐
   query text ───▶│   lexer.rs   │──▶ tokens (with line/column)
                  └──────────────┘
                          │
                  ┌──────────────┐
                  │  parser.rs   │──▶ ast.rs (Statement / Pattern / Expr)
                  └──────────────┘
                          │
                  ┌──────────────┐
                  │ executor.rs  │──▶ QueryResult { columns, rows, stats }
                  └──────────────┘
                     │        │
             ┌───────┘        └────────┐
      ┌──────────────┐       ┌──────────────┐
      │  graph.rs    │       │  value.rs    │
      │ nodes, edges │       │ types/order  │
      │ indexes      │       │ formatting   │
      └──────────────┘       └──────────────┘
             │
      ┌──────────────┐
      │  serial.rs   │  snapshot ⇄ query script
      └──────────────┘
```

Module responsibilities:

| File | Responsibility |
| --- | --- |
| `src/lexer.rs` | Tokens, string escapes, comments, source positions |
| `src/ast.rs` | Statements, patterns, expressions, display names |
| `src/parser.rs` | Recursive-descent parser and grammar errors |
| `src/graph.rs` | Property graph storage, indexes, ID allocation |
| `src/executor.rs` | Matching, mutation, projection, aggregation, functions |
| `src/value.rs` | Runtime values, comparisons, grouping keys, literals |
| `src/serial.rs` | Graph → Graphite script snapshots |
| `src/error.rs` | Positioned error type |
| `src/main.rs` | CLI, REPL, commands, table output |
| `tests/query.rs` | End-to-end language behavior |
| `tests/cli.rs` | Binary/CLI behavior |

---

## 4. Verified behavior

The test suite covers:

- Lexing identifiers, strings, arrows, comments, numbers, and error positions.
- Parsing match/create/set/delete and rejecting incomplete input.
- Graph indexing and refusing unsafe non-detached deletes.
- Creating nodes and relationships, including shared variables across patterns.
- Fixed and multi-hop traversal, reverse and undirected edges.
- Property matching on nodes and relationships.
- `WHERE`, `NOT`, `IS NULL`, three-valued logic, and filtering.
- `SET` with expressions and `RETURN` of updated values.
- `DELETE` and `DETACH DELETE`, including return-after-delete.
- Aggregation, grouping, `count(*)` on empty input, min/max/sum/avg.
- `DISTINCT`, `ORDER BY` (including aggregate expressions), `SKIP`, `LIMIT`.
- `RETURN *`, node/relationship values, `id`, `labels`, `type`, `keys`.
- Script execution and snapshot round-trip.
- CLI `-e`, `--help`, `--version`, exit codes, and positioned errors.

Run everything with:

```bash
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test
```

---

## 5. Explicit limitations of the MVP

### Language limitations

- No `OPTIONAL MATCH`, `MERGE`, `WITH`, `UNWIND`, `UNION`, `CASE`, `EXISTS`,
  subqueries, `CALL`, or procedure calls.
- No variable-length paths (`-[:KNOWS*1..3]->`), shortest path, or path values.
- No named paths or repeated-relationship constraints within a pattern.
- One relationship type per pattern (no `:KNOWS|LIKES`).
- No parameters (`$name`); all values are embedded in query text.
- No `MATCH` modifications beyond `SET`/`DELETE`; no `ON CREATE`/`ON MATCH`.
- Aggregates are limited to five functions; no `collect`, `count(DISTINCT x)`,
  percentile, standard deviation, or user-defined aggregates.
- No list/map indexing or comprehensions, no `IN`, `STARTS WITH`, `CONTAINS`,
  regex, `CASE`, or date/time functions.
- No `EXPLAIN` / `PROFILE`.
- No plan hints, index hints, or query parameters.
- Errors identify token positions but AST nodes do not carry spans.
- Keywords are case-insensitive, but labels/types/variables are case-sensitive.

### Transaction and consistency limitations

- Every statement mutates the graph directly. A failure halfway through a
  statement or script can leave partial changes.
- No rollback, savepoints, isolation levels, or MVCC.
- No concurrent access guarantees; `Database` requires `&mut self`.
- No unique constraints, existence constraints, or key constraints.
- No schema validation, property type enforcement, or cardinality checks.
- No statement timeouts, memory limits, or cancellation.

### Storage and operational limitations

- Entire graph lives in RAM; process exit without `:save` loses data.
- `:save` writes directly to the target file, not atomically (no temp+rename,
  no `fsync`).
- No WAL, no crash recovery, no incremental persistence, no file locking.
- No backup tooling, PITR, import from CSV/JSON, or streaming export.
- No multi-database, tenants, replication, sharding, or clustering.
- No server, authentication, authorization, TLS, audit logging, or quotas.
- No statistics, metrics, structured logs, health endpoints, or admin tooling.

### Performance and planning limitations

- No cost-based optimizer: patterns are executed in written order.
- No property index selection: `WHERE` filters scan labeled candidates.
- Adjacency entries are not indexed by relationship type.
- Matching clones rows and node/edge snapshots aggressively.
- Grouping builds string keys for every row.
- Single-threaded execution; no vectorization, parallelism, or query cache.
- No cardinality estimates, join ordering, or predicate pushdown.
- No memory accounting or spill-to-disk.
- REPL completeness checking reparses the buffer on each line.

### Type system limitations

- No date, time, datetime, duration, decimal, bytes, UUID, or geospatial types.
- Floats are IEEE-754 `f64`; no decimal exactness for money.
- No temporal arithmetic or timezone handling.
- No formal type coercion rules beyond a small numeric promotion.

---

## 6. What production graph databases have

The following is a capability model based on mature property-graph systems such
as Neo4j, Memgraph, TigerGraph, JanusGraph, Dgraph, NebulaGraph, ArangoDB,
Kùzu, FalkorDB, and Amazon Neptune. Not every system has every item, but this is
the feature surface a “real” graph database is expected to compete on.

### 6.1 Capability matrix

| Area | Graphite 0.1 | Production expectation | Gap severity |
| --- | --- | --- | --- |
| Property graph model | ✅ | ✅ | — |
| Multi-label nodes | ✅ | ✅ | — |
| Typed relationships | ✅ | ✅ | — |
| Fixed-length traversal | ✅ | ✅ | — |
| Variable-length traversal | ❌ | ✅ | High |
| Shortest path | ❌ | ✅ | High |
| Query language | Cypher-like subset | Cypher/GQL, often standards-track | High |
| Parameters | ❌ | ✅ | High |
| `OPTIONAL MATCH` / `MERGE` / `WITH` / `UNWIND` | ❌ | ✅ | High |
| Aggregations / `collect` | Partial | Rich | Medium |
| Query planner | Rule-free, no plan | Cost/rule-based, `EXPLAIN` | High |
| Property indexes | ❌ | Hash/range/composite | High |
| Full-text / spatial / vector indexes | ❌ | Often (varies) | Medium |
| Constraints / schema | ❌ | Unique, existence, type | High |
| ACID transactions | ❌ | ✅ | Critical |
| Rollback / isolation / MVCC | ❌ | ✅ | Critical |
| Durability | Snapshots only | WAL, recovery, fsync | Critical |
| Crash recovery | ❌ | ✅ | Critical |
| Concurrency | None | Thread-safe sessions | Critical |
| Network protocol | ❌ | Bolt/HTTP/gRPC/custom | High |
| Official drivers | ❌ | Multi-language | High |
| Auth / RBAC / TLS | ❌ | ✅ | High for servers |
| Multi-database / tenancy | ❌ | Common | Medium |
| Replication / sharding | ❌ | Enterprise/common | Medium–High at scale |
| Backup / PITR | ❌ | ✅ | High for ops |
| CSV/JSON import/export | ❌ | ✅ | Medium |
| Metrics / logs / tracing | ❌ | ✅ | Medium |
| Resource limits / timeouts | ❌ | ✅ | Medium |
| Graph algorithms library | ❌ | Neo4j GDS, TigerGraph, etc. | Medium |
| Visualization / browser | ❌ | ✅ | Medium |
| Cloud / managed offering | ❌ | ✅ | Low for MVP |
| Temporal / geospatial / vector types | ❌ | Varies | Medium |
| GQL standard compliance | ❌ | Emerging | Medium |

### 6.2 Language features leaders have

- **Pattern matching:** variable-length paths, quantified path patterns,
  shortest path, all-shortest paths, relationship uniqueness, path variables,
  named paths, pattern predicates, labels/type alternatives.
- **Clauses:** `OPTIONAL MATCH`, `MERGE` with `ON CREATE`/`ON MATCH`,
  `WITH` for query pipelines, `UNWIND`, `UNION`/`UNION ALL`, `CALL`,
  subqueries, `FOREACH`, `LOAD CSV`.
- **Expressions:** `CASE`, `EXISTS { ... }`, `IN`, pattern comprehensions,
  list/map comprehensions, slicing, map projection, `reduce`, `all/any/none`,
  string and numeric function libraries, temporal functions.
- **Aggregation:** `collect`, `count(DISTINCT)`, percentiles, collect distinct,
  ordered aggregation, grouping keys and `HAVING`-like filtering via `WITH`.
- **Parameters and safety:** server-side parameter binding, prepared statements,
  query plan caching, injection-resistant parameter handling.
- **Explainability:** `EXPLAIN`, `PROFILE`, query plans, operator metrics,
  runtime warnings, index hints.
- **Standards:** Cypher (openCypher), ISO GQL (ISO/IEC 39075:2024), and
  vendor-specific extensions.

### 6.3 Storage and transactions leaders have

- Write-ahead logging, group commit, `fsync` policies, checkpointing.
- Crash recovery, torn-write protection, and consistency checks.
- ACID transactions across multiple statements, explicit `BEGIN`/`COMMIT`/
  `ROLLBACK`, savepoints, and configurable isolation.
- MVCC or locking with readers not blocked by writers.
- Buffer pool, page cache, compression, memory mapping, and spill-to-disk.
- Columnar or hybrid storage for analytical workloads.
- Incremental backup, point-in-time recovery, and restore verification.
- Online index builds, index-only scans, and statistics collection.
- Multi-version schema changes and migration tooling.

### 6.4 Indexing and query planning leaders have

- Equality, range, composite, unique, and existence indexes.
- Full-text, geospatial, vector, and label-property indexes.
- Statistics and cardinality estimation.
- Join ordering, predicate pushdown, index selection, and scan vs seek costing.
- Adaptive execution, runtime re-optimization, and plan caching.
- Parallel and vectorized operators.
- Materialized views / graph projections for analytics.

### 6.5 Operations and platform leaders have

- Client/server architecture with connection pooling, sessions, and transactions.
- Authentication, RBAC, row/field-level controls, TLS, encryption at rest, audit
  logs.
- Multi-database, tenant isolation, quotas, and resource groups.
- Replication, failover, sharding, and distributed transactions.
- Metrics (Prometheus), structured logs, traces, health endpoints, and slow-query
  logs.
- Backup/restore CLI, PITR, upgrade tooling, and migration checks.
- Kubernetes operators, Helm charts, cloud-managed offerings.
- Drivers for Java, Python, JavaScript, Go, .NET, Rust, and more.
- Visualization, BI connectors, GraphQL, and streaming integrations.

### 6.6 Analytics and graph algorithms leaders have

- PageRank, community detection, centrality, similarity, link prediction.
- Graph projections and in-memory analytics workspaces.
- Pathfinding and network flow algorithms.
- Streaming graph updates and analytical query support.
- Integration with data warehouses and ML pipelines.

---

## 7. Prioritized gap analysis

### P0 — Make the single node trustworthy

These are non-negotiable before calling Graphite a usable database.

| # | Gap | Why it matters | Acceptance criteria |
| --- | --- | --- | --- |
| 1 | Statement atomicity + rollback | A failed `CREATE`/`SET` must not leave half a graph | Failed statement leaves byte-for-byte identical graph; tests for failure mid-pattern |
| 2 | WAL + durable commit | Snapshots alone lose acknowledged writes | Every committed mutation survives `kill -9`; replay test |
| 3 | Crash recovery + integrity check | A database must reopen correctly | Open after truncated/corrupt WAL cases; recovery report |
| 4 | Constraints + unique indexes | Duplicate identities corrupt application data | `CREATE CONSTRAINT ... UNIQUE`; violation fails atomically |
| 5 | Parameter binding | Safe, reusable, cacheable queries | `execute_with_params(query, params)`; no string interpolation |
| 6 | Transaction API | Multi-statement units of work | `begin/commit/rollback`, visible isolation behavior documented |
| 7 | Property indexes | `WHERE id = ...` must not scan labels | Equality/range index; `EXPLAIN` proves index use |
| 8 | `EXPLAIN` + basic planner | Users must understand and tune queries | Plan tree, estimated rows, actual rows, index decisions |
| 9 | Concurrency control | Real apps have concurrent readers/writers | `RwLock`/MVCC, no data races, documented isolation |
| 10 | Memory limits and timeouts | Prevent one query from exhausting the process | Configurable limits; cancellable long queries |

### P1 — Become a complete single-node graph database

| # | Gap | Details |
| --- | --- | --- |
| 11 | `OPTIONAL MATCH` | Left-outer pattern semantics with null bindings |
| 12 | `MERGE` | Idempotent upserts, `ON CREATE` / `ON MATCH` |
| 13 | `WITH` | Query pipelines, aggregation filtering, scope boundaries |
| 14 | `UNWIND` + `UNION` | List expansion and set operations |
| 15 | Variable-length paths | `*`, `*min..max`, path variables |
| 16 | Shortest path | `shortestPath`, `allShortestPaths`, weighted variant later |
| 17 | `CASE`, `EXISTS`, `IN`, pattern predicates | Practical query expressiveness |
| 18 | Richer aggregation | `collect`, `count(DISTINCT)`, ordering, percentiles |
| 19 | List/map/string function library | Usability for real data shaping |
| 20 | CSV/JSON import/export | Onboarding and interoperability |
| 21 | Query parameters in CLI | Reproducible scripts without interpolation |
| 22 | Network server + protocol | Session handling, request/response, streaming |
| 23 | Drivers | At least Python/JS/Rust clients |
| 24 | Auth/RBAC/TLS | Required as soon as there is a network |
| 25 | Metrics/logging/tracing | Operability |
| 26 | Backup/restore tooling | Safety and deployments |
| 27 | Multi-database | Namespace isolation |
| 28 | Full-text and vector indexes | Modern search and AI workloads |

### P2 — Scale and differentiate

| # | Gap | Details |
| --- | --- | --- |
| 29 | Replication + failover | HA, read replicas, consistent snapshots |
| 30 | Sharding / distributed execution | Partitioning, distributed traversals |
| 31 | MVCC + snapshot isolation | Concurrent long-running reads |
| 32 | Vectorized / parallel execution | Throughput and analytics |
| 33 | Graph algorithm library | PageRank, communities, centrality, similarity |
| 34 | Graph projections | Analytics workspaces separate from operational graph |
| 35 | Temporal, geospatial, decimal types | Domain modeling |
| 36 | GQL standard alignment | Portability and credibility |
| 37 | Cloud/operator | Managed deployment |
| 38 | Visualization / browser | Adoption and debugging |

---

## 8. Code-level improvements to make now

These are improvements to the current implementation, independent of new
features:

1. **Use IDs instead of cloned snapshots for bindings.**
   `Binding::Node(Node)` clones labels and every property per row. Store
   `NodeId`/`EdgeId` and borrow from the graph; this removes a major allocation
   hotspot.

2. **Replace `Vec<Row>` materialization between patterns.**
   `match_patterns` builds complete result sets per pattern. Use an iterator or
   callback pipeline so large matches do not multiply memory.

3. **Avoid `Vec::contains` in `incident_ids`.**
   Merge outgoing/incoming sorted iterators to deduplicate self-loops in
   `O(n)` instead of `O(n²)`.

4. **Add indexes for relationship type.**
   `outgoing: HashMap<NodeId, BTreeSet<EdgeId>>` filters by type after fetching
   all edges. Use `HashMap<(NodeId, Type), ...>` or a type component in the
   adjacency key.

5. **Add property indexes.**
   Equality and range indexes on `(label, property)` are the single biggest
   performance lever for read queries.

6. **Add spans to AST nodes.**
   Errors currently point at tokens only. AST spans enable “unknown variable”,
   “type mismatch”, and “property does not exist” diagnostics at the right place.

7. **Pre-validate queries before execution.**
   Resolve variables, aggregate placement, SET targets, and function arity during
   a semantic-analysis pass, so runtime cannot fail halfway through a write.

8. **Use hashable typed keys for grouping.**
   `Value::key()` allocates strings for every row. Implement `Eq + Hash` for a
   normalized key enum, or sort-based grouping.

9. **Make snapshots atomic.**
   Write to `path.tmp`, `fsync`, then rename. Optionally checksum and version the
   snapshot header.

10. **Avoid REPL reparsing.**
    Track quote/comment/semicolon state incrementally instead of calling
    `parse_program` on the whole buffer after every line.

11. **Separate execution from formatting.**
    The library returns `Value`; CLI table rendering is fine, but add a stable
    serialization layer (JSON/CSV) for tooling.

12. **Add fuzz and property-based tests.**
    Lexer/parser fuzzing for panics, plus property tests for
    serialize→load→serialize equality and arbitrary query execution.

13. **Add benchmarks.**
    Track parse time, match throughput, 1-hop/2-hop/3-hop traversal, point
    lookup, aggregation, and snapshot load/save. A `criterion`-based bench suite
    is appropriate once dependencies are allowed.

14. **Add logging/tracing.**
    Instrument parse, plan, execute, and storage phases; expose slow-query logs
    and operator-level counters.

15. **Define error taxonomy.**
    Separate syntax, semantic, constraint, execution, storage, and transaction
    errors so clients can react programmatically.

---

## 9. Recommended roadmap

### Milestone 2 — Durable, atomic single-node database

**Goal:** never lose an acknowledged write, never leave partial mutations.

- Transaction object with `commit`/`rollback`.
- Statement-level atomicity and pre-execution validation.
- WAL with checksums, group commit, replay, and checkpoints.
- Snapshot format versioning, atomic file replacement, `fsync`.
- Crash-recovery integration tests (truncated WAL, partial write, empty WAL).
- Constraints: unique node property, required property, relationship endpoints.
- Equality/range property indexes and `EXPLAIN`.
- Parameters and prepared statements.
- Concurrency: `RwLock` or copy-on-write snapshots with documented isolation.
- Benchmarks and load tests.

**Exit criteria:** 1M-node/5M-edge dataset loads, queries, crashes, recovers, and
passes invariants; no acknowledged write is lost.

### Milestone 3 — Language completeness

**Goal:** express the queries application developers actually write.

- `OPTIONAL MATCH`, `MERGE`, `WITH`, `UNWIND`, `UNION`.
- Variable-length paths and shortest path.
- `CASE`, `EXISTS`, `IN`, `STARTS WITH`, `CONTAINS`, list/map functions.
- `collect`, `count(DISTINCT)`, ordered aggregation.
- Pattern predicates, path variables, relationship uniqueness rules.
- Better diagnostics with AST spans and semantic analysis.
- Cypher compatibility test corpus.

### Milestone 4 — Server and ecosystem

**Goal:** usable from other processes and languages.

- Client/server protocol with sessions and streaming results.
- Auth, RBAC, TLS, audit logging.
- Python and JavaScript drivers; CLI parameter support.
- CSV/JSON import/export and bulk loader.
- Metrics, structured logs, health endpoints, slow-query log.
- Backup/restore and PITR.
- Multi-database and quotas.

### Milestone 5 — Scale and analytics

**Goal:** handle larger graphs and analytical workloads.

- MVCC/snapshot isolation, read replicas.
- Replication and failover; then sharding/distributed traversals.
- Vectorized/parallel operators and query caching.
- Graph algorithm library and graph projections.
- Vector/full-text/geospatial indexes.
- Temporal and decimal types.

### Milestone 6 — Standards and platform

**Goal:** portability and adoption.

- ISO GQL alignment and a conformance suite.
- Visualization/browser.
- Kubernetes operator / managed offering.
- BI/GraphQL/streaming integrations.

---

## 10. Suggested next 10 issues

1. Add a semantic-analysis pass and statement atomicity.
2. Add `Transaction` API with rollback.
3. Implement WAL append/fsync, replay, checkpoint, and recovery tests.
4. Version and atomically write snapshot files.
5. Add `CREATE CONSTRAINT ... UNIQUE` and enforcement.
6. Add parameter binding in the library and CLI.
7. Add equality/range property indexes and an `EXPLAIN` command.
8. Replace binding snapshots with IDs and remove row cloning.
9. Add `OPTIONAL MATCH` and `WITH`.
10. Add variable-length paths and `shortestPath`.

---

## 11. How to measure progress

Track these continuously:

| Dimension | Metric |
| --- | --- |
| Correctness | Test count, crash-recovery pass rate, invariant checks after every mutation |
| Performance | Parse µs, 1/2/3-hop traversals/sec, point lookup p99, aggregation throughput |
| Scale | Max nodes/edges under a fixed memory budget; snapshot/load time |
| Safety | Fuzz hours without panic; corruption tests passed; rollback equivalence |
| Operability | Mean recovery time, backup/restore time, query timeout behavior |
| Language | Percentage of openCypher/GQL conformance corpus passing |
| Quality | Clippy warnings, unsafe blocks, dependency audit, code coverage |

---

## 12. Bottom line

Milestone 1 proves the core idea end-to-end: Graphite owns its data model,
language, execution engine, and interface. It is a solid foundation for learning
and iteration.

The largest gaps versus production graph databases are not cosmetic:

1. **Durability and transactions** — without these it is an in-memory query
   engine, not a database.
2. **Query expressiveness** — variable-length paths, `OPTIONAL MATCH`, `MERGE`,
   `WITH`, parameters.
3. **Indexes and planning** — property indexes and a real optimizer.
4. **Operations** — server, auth, backups, metrics, concurrency.

The recommended sequence is therefore: **trustworthiness → expressiveness →
server/ecosystem → scale/analytics**. Adding more syntax before atomicity and
durability would increase the surface area of a system that can still lose data.
