use graphite::{Database, Value, DEMO_QUERY};

fn demo() -> Database {
    let mut db = Database::new();
    db.execute(DEMO_QUERY).unwrap();
    db
}

fn rows(db: &mut Database, query: &str) -> Vec<Vec<Value>> {
    db.execute(query).unwrap().into_iter().next().unwrap().rows
}

fn names(db: &mut Database, query: &str) -> Vec<String> {
    rows(db, query)
        .into_iter()
        .map(|row| match &row[0] {
            Value::String(name) => name.clone(),
            other => panic!("expected a string, found {other:?}"),
        })
        .collect()
}

#[test]
fn traverses_multiple_hops_in_both_directions() {
    let mut db = demo();

    let forward = rows(
        &mut db,
        r#"MATCH (a:Person)-[:KNOWS]->(b:Person)-[:KNOWS]->(c:Person)
           RETURN a.name, b.name, c.name ORDER BY a.name"#,
    );
    assert_eq!(forward.len(), 2);
    assert_eq!(forward[0][0], Value::String("Alice".into()));
    assert_eq!(forward[0][2], Value::String("Carol".into()));

    let backward = names(
        &mut db,
        r#"MATCH (b:Person)<-[:KNOWS]-(a:Person) WHERE b.name = "Carol"
           RETURN a.name ORDER BY a.name"#,
    );
    assert_eq!(backward, vec!["Bob"]);
}

#[test]
fn matches_undirected_relationships_and_filters_on_them() {
    let mut db = demo();

    let undirected = names(
        &mut db,
        r#"MATCH (a:Person)-[r:KNOWS]-(b:Person) WHERE a.name = "Bob" AND r.since >= 2019
           RETURN b.name ORDER BY b.name"#,
    );
    assert_eq!(undirected, vec!["Alice", "Carol"]);
}

#[test]
fn matches_labels_properties_and_relationship_types() {
    let mut db = demo();

    let result = rows(
        &mut db,
        r#"MATCH (p:Person {city: "Nairobi"})-[:WORKS_AT]->(c:Company)
           RETURN p.name, c.name ORDER BY p.name"#,
    );
    assert_eq!(result.len(), 2);
    assert_eq!(result[0][0], Value::String("Alice".into()));
    assert_eq!(result[0][1], Value::String("Acme".into()));
    assert_eq!(result[1][0], Value::String("Carol".into()));
    assert_eq!(result[1][1], Value::String("Orbit".into()));
}

#[test]
fn creates_patterns_with_shared_variables() {
    let mut db = Database::new();
    let result = rows(
        &mut db,
        r#"CREATE (a:Person {name: "Alice"}), (b:Person {name: "Bob"}),
                  (a)-[:KNOWS {since: 2024}]->(b)
           RETURN a.name, b.name"#,
    );

    assert_eq!(result.len(), 1);
    assert_eq!(result[0][0], Value::String("Alice".into()));
    assert_eq!(result[0][1], Value::String("Bob".into()));
    assert_eq!(db.graph().node_count(), 2);
    assert_eq!(db.graph().edge_count(), 1);
}

#[test]
fn sets_properties_and_returns_updated_values() {
    let mut db = demo();
    let result = rows(
        &mut db,
        r#"MATCH (p:Person) WHERE p.name = "Alice"
           SET p.age = p.age + 1, p.active = true
           RETURN p.age, p.active"#,
    );

    assert_eq!(result[0][0], Value::Int(31));
    assert_eq!(result[0][1], Value::Bool(true));
}

#[test]
fn deletes_relationships_and_detaches_nodes() {
    let mut db = demo();

    rows(
        &mut db,
        r#"MATCH (a:Person)-[r:KNOWS]->(b:Person) WHERE a.name = "Alice" DELETE r"#,
    );
    assert_eq!(db.graph().edge_count(), 6);

    rows(
        &mut db,
        r#"MATCH (p:Person) WHERE p.name = "Alice" DETACH DELETE p"#,
    );
    assert_eq!(db.graph().node_count(), 5);
    assert!(rows(
        &mut db,
        r#"MATCH (p:Person) WHERE p.name = "Alice" RETURN p"#
    )
    .is_empty());
}

#[test]
fn refuses_to_delete_a_connected_node_without_detach() {
    let mut db = demo();
    let error = db
        .execute(r#"MATCH (p:Person) WHERE p.name = "Alice" DELETE p"#)
        .unwrap_err();
    assert!(error.message.contains("DETACH DELETE"));
    assert_eq!(db.graph().node_count(), 6);
}

#[test]
fn counts_groups_and_aggregates() {
    let mut db = demo();

    let per_city = rows(
        &mut db,
        r#"MATCH (p:Person) RETURN p.city AS city, count(*) AS people ORDER BY people DESC"#,
    );
    assert_eq!(per_city.len(), 3);
    assert_eq!(per_city[0][0], Value::String("Nairobi".into()));
    assert_eq!(per_city[0][1], Value::Int(2));

    let stats = rows(
        &mut db,
        r#"MATCH (p:Person) RETURN count(*) AS people, avg(p.age) AS average,
                  min(p.age) AS youngest, max(p.age) AS oldest, sum(p.age) AS total"#,
    );
    assert_eq!(stats[0][0], Value::Int(4));
    assert_eq!(stats[0][1], Value::Float(33.25));
    assert_eq!(stats[0][2], Value::Int(27));
    assert_eq!(stats[0][3], Value::Int(41));
    assert_eq!(stats[0][4], Value::Int(133));
}

#[test]
fn aggregate_over_no_rows_returns_zero_count() {
    let mut db = demo();
    let result = rows(
        &mut db,
        "MATCH (p:Person) WHERE p.age > 100 RETURN count(*) AS total",
    );
    assert_eq!(result.len(), 1);
    assert_eq!(result[0][0], Value::Int(0));
}

#[test]
fn distinct_removes_duplicate_rows() {
    let mut db = demo();
    let cities = names(
        &mut db,
        "MATCH (p:Person) RETURN DISTINCT p.city ORDER BY p.city",
    );
    assert_eq!(cities, vec!["Kisumu", "Mombasa", "Nairobi"]);
}

#[test]
fn returns_all_bound_variables_with_star() {
    let mut db = demo();
    let result = db
        .execute(r#"MATCH (a:Person)-[r:KNOWS]->(b:Person) WHERE a.name = "Alice" RETURN *"#)
        .unwrap()
        .remove(0);

    assert_eq!(result.columns, vec!["a", "b", "r"]);
    assert_eq!(result.rows.len(), 1);
    assert!(matches!(result.rows[0][0], Value::Node(_)));
    assert!(matches!(result.rows[0][2], Value::Edge(_)));
}

#[test]
fn supports_identity_label_and_type_functions() {
    let mut db = demo();
    let result = rows(
        &mut db,
        r#"MATCH (a:Person)-[r:KNOWS]->(b:Person) WHERE a.name = "Alice"
           RETURN id(a) AS id, labels(a) AS labels, type(r) AS rel, keys(r) AS props"#,
    );

    assert_eq!(result[0][0], Value::Int(1));
    assert_eq!(
        result[0][1],
        Value::List(vec![Value::String("Person".into())])
    );
    assert_eq!(result[0][2], Value::String("KNOWS".into()));
    assert_eq!(
        result[0][3],
        Value::List(vec![Value::String("since".into())])
    );
}

#[test]
fn orders_aggregates_by_the_aggregate_expression() {
    let mut db = demo();
    let result = rows(
        &mut db,
        "MATCH (p:Person) RETURN p.city AS city, count(*) ORDER BY count(*) DESC, city",
    );

    assert_eq!(result.len(), 3);
    assert_eq!(result[0][0], Value::String("Nairobi".into()));
    assert_eq!(result[0][1], Value::Int(2));
}

#[test]
fn handles_null_checks_and_three_valued_filters() {
    let mut db = Database::new();
    db.execute(r#"CREATE (:Item {name: "has", price: 10}), (:Item {name: "missing"})"#)
        .unwrap();

    let missing = names(
        &mut db,
        "MATCH (i:Item) WHERE i.price IS NULL RETURN i.name",
    );
    assert_eq!(missing, vec!["missing"]);

    let present = names(
        &mut db,
        "MATCH (i:Item) WHERE i.price IS NOT NULL AND NOT i.price < 5 RETURN i.name",
    );
    assert_eq!(present, vec!["has"]);

    // Three-valued logic: null propagates through AND/OR/NOT, and WHERE only
    // passes rows that are literally true.
    let logic = rows(
        &mut db,
        "MATCH (i:Item) WHERE i.name = \"missing\"
         RETURN i.price AND true AS a, i.price OR true AS b,
                i.price OR false AS c, i.price AND false AS d, NOT i.price AS e",
    );
    assert_eq!(logic[0][0], Value::Null);
    assert_eq!(logic[0][1], Value::Bool(true));
    assert_eq!(logic[0][2], Value::Null);
    assert_eq!(logic[0][3], Value::Bool(false));
    assert_eq!(logic[0][4], Value::Null);

    let filtered = rows(
        &mut db,
        "MATCH (i:Item) WHERE i.price AND true RETURN i.name",
    );
    assert!(filtered.is_empty());
}

#[test]
fn orders_skips_and_limits() {
    let mut db = demo();
    let result = names(
        &mut db,
        "MATCH (p:Person) RETURN p.name ORDER BY p.age DESC SKIP 1 LIMIT 2",
    );
    assert_eq!(result, vec!["Carol", "Alice"]);
}

#[test]
fn scripts_run_multiple_statements_in_order() {
    let mut db = Database::new();
    let results = db
        .execute(
            r#"
            CREATE (:Counter {value: 1});
            MATCH (c:Counter) SET c.value = c.value + 1;
            MATCH (c:Counter) RETURN c.value;
            "#,
        )
        .unwrap();

    assert_eq!(results.len(), 3);
    assert_eq!(results[2].rows[0][0], Value::Int(2));
}

#[test]
fn reports_useful_errors() {
    let mut db = Database::new();

    let syntax = db.execute("MATCH (n) RETURN").unwrap_err();
    assert!(syntax.message.contains("expected an expression"));

    db.execute(r#"CREATE (a:Person {name: "A"})-[:KNOWS]->(b:Person {name: "B"})"#)
        .unwrap();
    let delete = db.execute("MATCH (n:Person) DELETE n").unwrap_err();
    assert!(delete.message.contains("DETACH DELETE"));

    let function = db.execute("MATCH (n) RETURN mystery(n)").unwrap_err();
    assert!(function.message.contains("unknown function"));
}

#[test]
fn create_can_return_the_new_nodes() {
    let mut db = Database::new();
    let result = db
        .execute(r#"CREATE (n:Thing {name: "new"}) RETURN n.name, labels(n)"#)
        .unwrap()
        .remove(0);

    assert_eq!(result.rows[0][0], Value::String("new".into()));
    assert_eq!(
        result.rows[0][1],
        Value::List(vec![Value::String("Thing".into())])
    );
    assert_eq!(result.stats.nodes_created, 1);
}

#[test]
fn handles_detach_delete_with_return() {
    let mut db = Database::new();
    db.execute(r#"CREATE (a:Person {name: "A"})-[:KNOWS]->(b:Person {name: "B"})"#)
        .unwrap();

    let result = rows(
        &mut db,
        r#"MATCH (a:Person {name: "A"}) DETACH DELETE a RETURN a.name"#,
    );
    assert_eq!(result[0][0], Value::String("A".into()));
    assert_eq!(db.graph().node_count(), 1);
    assert_eq!(db.graph().edge_count(), 0);
}
