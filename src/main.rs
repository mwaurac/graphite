use std::env;
use std::fs;
use std::io::{self, BufRead, IsTerminal, Read, Write};
use std::process::ExitCode;

use graphite::{parser::parse_program, Database, QueryResult, DEMO_QUERY};

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let mut directory: Option<String> = None;
    let mut action: Option<Action> = None;

    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "-h" | "--help" => {
                print_help();
                return ExitCode::SUCCESS;
            }
            "-V" | "--version" => {
                println!("graphite {VERSION}");
                return ExitCode::SUCCESS;
            }
            "--db" => {
                let Some(path) = args.get(index + 1) else {
                    eprintln!("error: --db requires a directory");
                    return ExitCode::from(2);
                };
                if directory.replace(path.clone()).is_some() {
                    eprintln!("error: --db can only be specified once");
                    return ExitCode::from(2);
                }
                index += 2;
            }
            "-e" | "--execute" => {
                let Some(query) = args.get(index + 1) else {
                    eprintln!("error: {} requires a query argument", args[index]);
                    return ExitCode::from(2);
                };
                if action.is_some() {
                    eprintln!("error: only one of --execute or --file may be used");
                    return ExitCode::from(2);
                }
                action = Some(Action::Execute(query.clone()));
                index += 2;
            }
            "-f" | "--file" => {
                let Some(path) = args.get(index + 1) else {
                    eprintln!("error: {} requires a file path", args[index]);
                    return ExitCode::from(2);
                };
                if action.is_some() {
                    eprintln!("error: only one of --execute or --file may be used");
                    return ExitCode::from(2);
                }
                action = Some(Action::File(path.clone()));
                index += 2;
            }
            other if other.starts_with('-') => {
                eprintln!("error: unknown option `{other}`");
                eprintln!("run `graphite --help` for usage");
                return ExitCode::from(2);
            }
            path => {
                if action.is_some() {
                    eprintln!("error: only one script file may be provided");
                    return ExitCode::from(2);
                }
                action = Some(Action::File(path.to_string()));
                index += 1;
            }
        }
    }

    let mut db = match &directory {
        Some(directory) => match Database::open(directory) {
            Ok(db) => db,
            Err(error) => {
                eprintln!("error: {error}");
                return ExitCode::from(1);
            }
        },
        None => Database::new(),
    };

    match action {
        Some(Action::Execute(query)) => run_source(&mut db, &query),
        Some(Action::File(path)) => run_file(&mut db, &path),
        None => {
            if io::stdin().is_terminal() {
                return repl(&mut db);
            }
            let mut source = String::new();
            if let Err(error) = io::stdin().read_to_string(&mut source) {
                eprintln!("error: failed to read stdin: {error}");
                return ExitCode::from(1);
            }
            run_source(&mut db, &source)
        }
    }
}

enum Action {
    Execute(String),
    File(String),
}

fn run_file(db: &mut Database, path: &str) -> ExitCode {
    match fs::read_to_string(path) {
        Ok(source) => run_source(db, &source),
        Err(error) => {
            eprintln!("error: cannot read `{path}`: {error}");
            ExitCode::from(1)
        }
    }
}

fn run_source(db: &mut Database, source: &str) -> ExitCode {
    match db.execute(source) {
        Ok(results) => {
            for result in results {
                print_result(&result);
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::from(1)
        }
    }
}

fn print_result(result: &QueryResult) {
    if !result.columns.is_empty() {
        print_table(&result.columns, &result.rows);
    }

    let summary = result.summary();
    if !summary.is_empty() {
        println!("{summary}");
    } else if result.columns.is_empty() {
        if result.rows.is_empty() {
            println!("OK");
        } else {
            let label = if result.rows.len() == 1 {
                "row"
            } else {
                "rows"
            };
            println!("{} {label}", result.rows.len());
        }
    }
}

fn print_table(columns: &[String], rows: &[Vec<graphite::Value>]) {
    let cells: Vec<Vec<String>> = rows
        .iter()
        .map(|row| row.iter().map(|value| value.literal()).collect())
        .collect();

    let mut widths: Vec<usize> = columns.iter().map(String::len).collect();
    for row in &cells {
        for (index, cell) in row.iter().enumerate() {
            if index < widths.len() {
                widths[index] = widths[index].max(cell.chars().count());
            }
        }
    }

    let separator: String = widths
        .iter()
        .map(|width| "-".repeat(width + 2))
        .collect::<Vec<_>>()
        .join("+");
    let separator = format!("+{separator}+");

    println!("{separator}");
    let header: Vec<String> = columns
        .iter()
        .enumerate()
        .map(|(index, column)| format!(" {:<width$} ", column, width = widths[index]))
        .collect();
    println!("|{}|", header.join("|"));
    println!("{separator}");

    for row in &cells {
        let line: Vec<String> = row
            .iter()
            .enumerate()
            .map(|(index, cell)| format!(" {:<width$} ", cell, width = widths[index]))
            .collect();
        println!("|{}|", line.join("|"));
    }
    println!("{separator}");

    let label = if rows.len() == 1 { "row" } else { "rows" };
    println!("{} {label}", rows.len());
}

fn repl(db: &mut Database) -> ExitCode {
    println!("Graphite {VERSION} — a small graph database");
    println!("Type `:help` for help, `:demo` for sample data, `:quit` to exit.");
    println!();

    let stdin = io::stdin();
    let mut input = stdin.lock();
    let mut buffer = String::new();

    loop {
        let prompt = if buffer.trim().is_empty() {
            "graphite> "
        } else {
            "       ... "
        };
        print!("{prompt}");
        let _ = io::stdout().flush();

        let mut line = String::new();
        match input.read_line(&mut line) {
            Ok(0) => {
                println!();
                return ExitCode::SUCCESS;
            }
            Ok(_) => {}
            Err(error) => {
                eprintln!("error: failed to read input: {error}");
                return ExitCode::from(1);
            }
        }

        let trimmed = line.trim();
        if buffer.trim().is_empty() && trimmed.starts_with(':') {
            match run_command(db, trimmed) {
                CommandOutcome::Continue => continue,
                CommandOutcome::Quit => return ExitCode::SUCCESS,
            }
        }

        buffer.push_str(&line);

        if buffer.trim().is_empty() {
            buffer.clear();
            continue;
        }

        if statement_complete(&buffer) || trimmed.is_empty() {
            let source = std::mem::take(&mut buffer);
            if !source.trim().is_empty() {
                run_source(db, &source);
            }
        }
    }
}

enum CommandOutcome {
    Continue,
    Quit,
}

fn run_command(db: &mut Database, command: &str) -> CommandOutcome {
    let mut parts = command.split_whitespace();
    match parts.next().unwrap_or("") {
        ":quit" | ":exit" | ":q" => return CommandOutcome::Quit,
        ":help" | ":h" | ":?" => print_repl_help(),
        ":clear" => match replace_database(db, "") {
            Ok(()) => println!("Database cleared."),
            Err(error) => eprintln!("error: {error}"),
        },
        ":demo" => match replace_database(db, DEMO_QUERY) {
            Ok(()) => println!(
                "Loaded demo graph: {} nodes, {} relationships.",
                db.graph().node_count(),
                db.graph().edge_count()
            ),
            Err(error) => eprintln!("error: {error}"),
        },
        ":stats" => {
            let mut line = format!(
                "{} nodes, {} relationships",
                db.graph().node_count(),
                db.graph().edge_count()
            );
            if let Some(directory) = db.directory() {
                line.push_str(&format!(
                    ", sequence {} (persistent at {})",
                    db.last_sequence(),
                    directory.display()
                ));
            }
            println!("{line}");
        }
        ":checkpoint" => match db.checkpoint() {
            Ok(()) => {
                if db.directory().is_some() {
                    println!("Checkpointed at sequence {}.", db.last_sequence());
                } else {
                    println!("Nothing to checkpoint: this database is in memory.");
                }
            }
            Err(error) => eprintln!("error: {error}"),
        },
        ":save" => {
            let path = command_argument(command);
            match path {
                None => eprintln!("usage: :save <file>"),
                Some(path) => match fs::write(path, db.dump()) {
                    Ok(()) => println!(
                        "Saved {} nodes and {} relationships to `{path}`.",
                        db.graph().node_count(),
                        db.graph().edge_count()
                    ),
                    Err(error) => eprintln!("error: cannot write `{path}`: {error}"),
                },
            }
        }
        ":load" => {
            let path = command_argument(command);
            match path {
                None => eprintln!("usage: :load <file>"),
                Some(path) => match fs::read_to_string(path) {
                    Ok(source) => match replace_database(db, &source) {
                        Ok(()) => println!(
                            "Loaded {} nodes and {} relationships from `{path}`.",
                            db.graph().node_count(),
                            db.graph().edge_count()
                        ),
                        Err(error) => eprintln!("error: {error}"),
                    },
                    Err(error) => eprintln!("error: cannot read `{path}`: {error}"),
                },
            }
        }
        other => {
            eprintln!("unknown command `{other}`; type `:help`");
        }
    }
    CommandOutcome::Continue
}

fn replace_database(db: &mut Database, source: &str) -> graphite::Result<()> {
    let statements = parse_program(source)?;
    let mut transaction = db.begin();
    transaction.execute("MATCH (n) DETACH DELETE n")?;
    if !statements.is_empty() {
        transaction.execute(source)?;
    }
    transaction.commit()
}

fn command_argument(command: &str) -> Option<&str> {
    let (_, argument) = command.split_once(char::is_whitespace)?;
    let argument = argument.trim();
    if argument.is_empty() {
        return None;
    }
    Some(
        argument
            .strip_prefix('"')
            .and_then(|rest| rest.strip_suffix('"'))
            .unwrap_or(argument),
    )
}

fn statement_complete(buffer: &str) -> bool {
    if buffer.trim_end().ends_with(';') {
        return true;
    }
    parse_program(buffer).is_ok()
}

fn print_repl_help() {
    println!("Commands:");
    println!("  :help              show this help");
    println!("  :demo              reset and load a small sample graph");
    println!("  :stats             show node and relationship counts");
    println!("  :checkpoint        snapshot a persistent database and clear its WAL");
    println!("  :save <file>       save the graph as a Graphite script");
    println!("  :load <file>       replace the graph with a saved script");
    println!("  :clear             remove all data");
    println!("  :quit              exit the shell");
    println!();
    println!("End a statement with `;` or press Enter on an empty line to run it.");
    println!();
    println!("Example queries:");
    println!(r#"  CREATE (a:Person {{name: "Alice"}})-[:KNOWS]->(b:Person {{name: "Bob"}});"#);
    println!(r#"  MATCH (n:Person) RETURN n.name, n.age ORDER BY n.name LIMIT 10;"#);
    println!(r#"  MATCH (a:Person)-[:KNOWS]->(b:Person) RETURN a.name, b.name;"#);
    println!(r#"  MATCH (n:Person) WHERE n.age > 30 SET n.older = true RETURN n.name;"#);
    println!(r#"  MATCH (n:Person) DETACH DELETE n;"#);
}

fn print_help() {
    println!("Graphite {VERSION} — a small graph database with its own query language.");
    println!();
    println!("USAGE:");
    println!("  graphite                    start the interactive shell");
    println!("  graphite <file.gql>         run statements from a file");
    println!("  graphite -e <query>         run a query");
    println!("  graphite -f <file.gql>      run statements from a file");
    println!("  graphite --db <dir>         open or create a persistent database");
    println!("  graphite --db <dir> -e ...  run a query against a persistent database");
    println!("  graphite --help             show this help");
    println!();
    println!("Without --db, data lives in memory. With --db, committed transactions");
    println!("are recorded in <dir>/wal.log and recovered on startup.");
}
