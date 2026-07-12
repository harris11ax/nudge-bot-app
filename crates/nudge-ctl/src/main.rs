//! nudge-ctl: config + status CLI. Read verbs: status, log, validate. Control:
//! quit, reload. Edit verbs (format-preserving, validated before write, then
//! auto-signal a running svc to reload): anchor, nudge add/rm/ls.
//!
//! Edits apply immediately on the running svc's next reload.

use std::path::PathBuf;

use toml_edit::{value, Array, ArrayOfTables, Document, Item, Table};

fn config_dir() -> PathBuf {
    PathBuf::from(std::env::var("LOCALAPPDATA").expect("LOCALAPPDATA")).join("nudge-bot")
}

fn rules_path() -> PathBuf {
    config_dir().join("rules.toml")
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let rest = &args.get(1..).unwrap_or(&[]);
    match args.first().map(String::as_str) {
        Some("status") => status(),
        Some("log") => log(args.get(1).and_then(|n| n.parse().ok()).unwrap_or(20)),
        Some("validate") => validate(args.get(1).map(PathBuf::from)),
        Some("quit") => quit(),
        Some("reload") => {
            if signal_reload() {
                println!("reload signal sent");
            } else {
                eprintln!("nudge-svc not running (no reload event)");
                std::process::exit(1);
            }
        }
        Some("anchor") => anchor(rest),
        Some("nudge") => nudge(rest),
        _ => usage(),
    }
}

fn usage() -> ! {
    eprintln!(
        "usage: nudge-ctl <command>\n\
         \n  status                                 last logged session state\
         \n  log [n]                                last n edge-log rows (default 20)\
         \n  validate [rules.toml]                  parse-check a rules file\
         \n  reload                                 tell running svc to re-read rules.toml\
         \n  quit                                   stop a running svc gracefully\
         \n  anchor <text...>                       set the anchor default text\
         \n  nudge ls                               list nudge windows\
         \n  nudge add <name> <days> <start> <end> <text...>\
         \n                                         add a window (days = mon,tue,...; times HH:MM)\
         \n  nudge rm <name>                        remove a window by name"
    );
    std::process::exit(2);
}

fn open_db() -> rusqlite::Connection {
    rusqlite::Connection::open_with_flags(
        config_dir().join("sessions.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .expect("sessions.db (is nudge-svc installed?)")
}

fn status() {
    let db = open_db();
    let row: Result<(i64, String), _> = db.query_row(
        "SELECT at, state FROM sessions ORDER BY at DESC LIMIT 1",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    );
    match row {
        Ok((at, state)) => println!("{state} since unix {at}"),
        Err(_) => println!("Idle (no session log yet)"),
    }
}

fn log(n: usize) {
    let db = open_db();
    let mut stmt = db
        .prepare("SELECT at, state FROM sessions ORDER BY at DESC LIMIT ?1")
        .unwrap();
    let rows = stmt
        .query_map([n], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))
        .unwrap();
    for row in rows {
        let (at, state) = row.unwrap();
        println!("{at}\t{state}");
    }
}

/// Open a named session-local event by name and SetEvent it. Returns false if
/// the event does not exist (svc not running). Mirrors the service's named
/// events — keep the literals in sync with shutdown.rs / reload.rs.
fn set_named_event(name: windows::core::PCWSTR) -> bool {
    use windows::Win32::Foundation::{CloseHandle, BOOL};
    use windows::Win32::System::Threading::{OpenEventW, SetEvent, EVENT_MODIFY_STATE};
    unsafe {
        match OpenEventW(EVENT_MODIFY_STATE, BOOL(0), name) {
            Ok(h) => {
                SetEvent(h).expect("SetEvent");
                let _ = CloseHandle(h);
                true
            }
            Err(_) => false,
        }
    }
}

/// Signal a running nudge-svc to shut down gracefully (see shutdown.rs).
fn quit() {
    if set_named_event(windows::core::w!("Local\\nudge-bot-quit")) {
        println!("quit signal sent");
    } else {
        eprintln!("nudge-svc not running (no quit event)");
        std::process::exit(1);
    }
}

/// Signal a running nudge-svc to re-read rules.toml (see reload.rs). No-op
/// (returns false) if svc is not running — edits still land on disk.
fn signal_reload() -> bool {
    set_named_event(windows::core::w!("Local\\nudge-bot-reload"))
}

fn validate(path: Option<PathBuf>) {
    let path = path.unwrap_or_else(rules_path);
    let src = std::fs::read_to_string(&path).expect("read rules file");
    match nudge_core::rules::parse(&src) {
        Ok(r) => println!("OK: {} nudge window(s)", r.nudges.len()),
        Err(e) => {
            eprintln!("INVALID: {e:?}");
            std::process::exit(1);
        }
    }
}

// --- Edit verbs -----------------------------------------------------------

/// Read rules.toml into a format-preserving document (keeps comments/layout).
fn load_doc() -> Document {
    let src = std::fs::read_to_string(rules_path())
        .expect("rules.toml missing — copy rules.example.toml");
    src.parse::<Document>().unwrap_or_else(|e| {
        eprintln!("rules.toml is not valid TOML: {e}");
        std::process::exit(1);
    })
}

/// Validate the mutated document against the full rules schema, then write it
/// atomically (tmp + rename) and signal a running svc to reload.
fn commit(doc: Document) {
    let rendered = doc.to_string();
    if let Err(e) = nudge_core::rules::parse(&rendered) {
        eprintln!("edit rejected (would produce invalid rules): {e:?}");
        std::process::exit(1);
    }
    let path = rules_path();
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, rendered.as_bytes()).expect("write temp rules");
    std::fs::rename(&tmp, &path).expect("replace rules.toml");
    if signal_reload() {
        println!("saved; running svc reloaded");
    } else {
        println!("saved (svc not running; applies on next start)");
    }
}

fn anchor(args: &[String]) {
    if args.is_empty() {
        eprintln!("usage: nudge-ctl anchor <text...>");
        std::process::exit(2);
    }
    let text = args.join(" ");
    let mut doc = load_doc();
    doc["anchor"]["default_text"] = value(text);
    commit(doc);
}

fn nudge(args: &[String]) {
    match args.first().map(String::as_str) {
        Some("ls") => nudge_ls(),
        Some("add") => nudge_add(&args[1..]),
        Some("rm") => nudge_rm(args.get(1).map(String::as_str)),
        _ => {
            eprintln!("usage: nudge-ctl nudge <ls|add|rm>");
            std::process::exit(2);
        }
    }
}

fn nudge_ls() {
    let doc = load_doc();
    let Some(aot) = doc.get("nudge").and_then(Item::as_array_of_tables) else {
        println!("(no nudge windows)");
        return;
    };
    for t in aot.iter() {
        let s = |k| t.get(k).and_then(Item::as_str).unwrap_or("?");
        let days = t
            .get("days")
            .and_then(Item::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str())
                    .collect::<Vec<_>>()
                    .join(",")
            })
            .unwrap_or_default();
        println!("{}  {}  {}-{}  {:?}", s("name"), days, s("start"), s("end"), s("text"));
    }
}

fn nudge_add(args: &[String]) {
    // name days start end text...
    if args.len() < 5 {
        eprintln!("usage: nudge-ctl nudge add <name> <days> <start> <end> <text...>");
        std::process::exit(2);
    }
    let (name, days_csv, start, end) = (&args[0], &args[1], &args[2], &args[3]);
    let text = args[4..].join(" ");

    let mut doc = load_doc();
    let aot = doc
        .entry("nudge")
        .or_insert(Item::ArrayOfTables(ArrayOfTables::new()))
        .as_array_of_tables_mut()
        .expect("[[nudge]] is not an array of tables");

    if aot
        .iter()
        .any(|t| t.get("name").and_then(Item::as_str) == Some(name.as_str()))
    {
        eprintln!("a nudge window named '{name}' already exists (rm it first)");
        std::process::exit(1);
    }

    let mut days = Array::new();
    for d in days_csv.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        days.push(d);
    }

    let mut t = Table::new();
    t["name"] = value(name.as_str());
    t["days"] = value(days);
    t["start"] = value(start.as_str());
    t["end"] = value(end.as_str());
    t["text"] = value(text);
    aot.push(t);

    commit(doc);
}

fn nudge_rm(name: Option<&str>) {
    let Some(name) = name else {
        eprintln!("usage: nudge-ctl nudge rm <name>");
        std::process::exit(2);
    };
    let mut doc = load_doc();
    let Some(aot) = doc.get_mut("nudge").and_then(Item::as_array_of_tables_mut) else {
        eprintln!("no nudge windows to remove");
        std::process::exit(1);
    };
    let before = aot.len();
    // Remove matching tables by index (descending to keep indices valid).
    let victims: Vec<usize> = (0..aot.len())
        .filter(|&i| aot.get(i).and_then(|t| t.get("name")).and_then(Item::as_str) == Some(name))
        .collect();
    for &i in victims.iter().rev() {
        aot.remove(i);
    }
    if aot.len() == before {
        eprintln!("no nudge window named '{name}'");
        std::process::exit(1);
    }
    commit(doc);
}
