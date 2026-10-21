use std::env;
use std::fs;
use std::io::{self, BufRead, IsTerminal, Read, Write};
use std::process::ExitCode;

use graphite::{parser::parse_program, Database, QueryResult, DEMO_QUERY};

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let mut db = Database::new();

    if args.is_empty() {
        if io::stdin().is_terminal() {
            return repl(&mut db);
        }
        let mut source = String::new();
        if let Err(error) = io::stdin().read_to_string(&mut source) {
            eprintln!("error: failed to read stdin: {error}");
            return ExitCode::from(1);
        }
        return run_source(&mut db, &source);
    }

    match args[0].as_str() {
        "-h" | "--help" => {
            print_help();
            ExitCode::SUCCESS
        }
        "-V" | "--version" => {
            println!("graphite {VERSION}");
            ExitCode::SUCCESS
        }
        "-e" | "--execute" => {
            let Some(query) = args.get(1) else {
                eprintln!("error: {} requires a query argument", args[0]);
                return ExitCode::from(2);
            };
            run_source(&mut db, query)
        }
        "-f" | "--file" => {
            let Some(path) = args.get(1) else {
                eprintln!("error: {} requires a file path", args[0]);
                return ExitCode::from(2);
            };
            run_file(&mut db, path)
        }
        other if other.starts_with('-') => {
            eprintln!("error: unknown option `{other}`");
            eprintln!("run `graphite --help` for usage");
            ExitCode::from(2)
        }
        path => run_file(&mut db, path),
    }
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
        ":clear" => {
            *db = Database::new();
            println!("Database cleared.");
        }
        ":demo" => {
            *db = Database::new();
            match db.execute(DEMO_QUERY) {
                Ok(_) => println!(
                    "Loaded demo graph: {} nodes, {} relationships.",
                    db.graph().node_count(),
                    db.graph().edge_count()
                ),
                Err(error) => eprintln!("error: {error}"),
            }
        }
        ":stats" => {
            println!(
                "{} nodes, {} relationships",
                db.graph().node_count(),
                db.graph().edge_count()
            );
        }
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
                    Ok(source) => match Database::from_script(&source) {
                        Ok(loaded) => {
                            *db = loaded;
                            println!(
                                "Loaded {} nodes and {} relationships from `{path}`.",
                                db.graph().node_count(),
                                db.graph().edge_count()
                            );
                        }
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
    println!("  graphite --help             show this help");
    println!();
    println!("Data lives in memory for this milestone.");
}
