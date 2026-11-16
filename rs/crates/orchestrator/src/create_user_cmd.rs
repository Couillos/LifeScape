#![forbid(unsafe_code)]
//! `orchestrator create-user [pseudo] [--iron] [--goal kind]
//! [--role market_maker] [--no-tutorial]` (split from `cli.rs`, rule 4):
//! the documented account procedure. The life file is written
//! (`brain::create_user`: natural pseudo when omitted, profile, goal,
//! the role's slice and capital target), then the session opens and the
//! brain's tutorial gate starts the tutorial, then the goal.
//! `--role market_maker` (plan marché Q6): a dedicated market maker on
//! the next free slice, playing until its capital target, then quoting
//! for good at its post by the Varrock fountain.

use lifescape_json::{self as json, object, str_value, Value};

const USAGE: &str = "create-user [pseudo] [--iron] [--goal kind] [--role market_maker] [--no-tutorial]";

/// Parsed `create-user` arguments (everything after the command).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CreateArgs {
    pub account: Option<String>,
    pub iron: bool,
    pub goal: Option<String>,
    pub role: Option<String>,
    pub no_tutorial: bool,
}

/// Parse `create-user` arguments: an optional pseudo first, then flags.
pub fn parse_create_user(args: &[String]) -> Result<CreateArgs, String> {
    let mut parsed = CreateArgs::default();
    let mut index = 0;
    if let Some(first) = args.first().filter(|first| !first.starts_with("--") && !first.is_empty()) {
        parsed.account = Some(first.clone());
        index = 1;
    }
    while index < args.len() {
        match args[index].as_str() {
            "--iron" => parsed.iron = true,
            "--no-tutorial" => parsed.no_tutorial = true,
            "--goal" | "--role" => {
                let value = args.get(index + 1).filter(|value| !value.starts_with("--")).cloned().ok_or_else(|| format!("{} needs a value (usage: {USAGE})", args[index]))?;
                if args[index] == "--goal" {
                    parsed.goal = Some(value);
                } else {
                    parsed.role = Some(value);
                }
                index += 1;
            }
            other => return Err(format!("unknown flag {other:?} (usage: {USAGE})")),
        }
        index += 1;
    }
    Ok(parsed)
}

/// The command: life file, then `open` through the daemon (`send`).
pub fn create_user_cli(args: &[String], send: &dyn Fn(Value) -> Value) -> i32 {
    let parsed = match parse_create_user(args) {
        Ok(parsed) => parsed,
        Err(error) => {
            eprintln!("create-user: {error}");
            return 2;
        }
    };
    let name = match crate::brain::create_user(parsed.account, parsed.iron, parsed.goal, parsed.role) {
        Ok(name) => name,
        Err(error) => {
            eprintln!("create-user: {error}");
            return 1;
        }
    };
    println!("created {name}");
    if parsed.no_tutorial {
        return 0;
    }
    // Open only: the brain's tutorial gate launches the tutorial as a
    // graph node (reported, failure-bounded) at the first idle pass with
    // a live position. One path, no race with a CLI-side `run` that
    // nothing would report.
    let reply = send(object(vec![("cmd", str_value("open")), ("account", str_value(&name))]));
    println!("{}", json::stringify_pretty(&reply));
    if reply.get("ok").as_bool() == Some(true) {
        println!("{name}: the brain starts the tutorial (gate) then the goal");
        0
    } else {
        1
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_create_user, CreateArgs};

    fn strings(raw: &[&str]) -> Vec<String> {
        raw.iter().map(|text| text.to_string()).collect()
    }

    #[test]
    fn the_role_flag_parses_beside_the_others() {
        assert_eq!(
            parse_create_user(&strings(&["--role", "market_maker"])),
            Ok(CreateArgs { role: Some("market_maker".to_string()), ..CreateArgs::default() }),
            "natural pseudo generated, maker role"
        );
        assert_eq!(
            parse_create_user(&strings(&["nina_rhodes", "--goal", "money", "--no-tutorial"])),
            Ok(CreateArgs { account: Some("nina_rhodes".to_string()), goal: Some("money".to_string()), no_tutorial: true, ..CreateArgs::default() })
        );
        assert!(parse_create_user(&strings(&["--iron"])).is_ok_and(|parsed| parsed.iron && parsed.account.is_none()));
        assert!(parse_create_user(&strings(&["--role"])).is_err(), "a dangling role fails loud");
        assert!(parse_create_user(&strings(&["--role", "--iron"])).is_err());
        assert!(parse_create_user(&strings(&["--merchant"])).is_err(), "unknown flags fail loud");
    }
}
