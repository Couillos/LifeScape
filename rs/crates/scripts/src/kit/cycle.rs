//! Cycle: task execution contract (L2a: minimal `TaskSpec` + the common
//! CLI parser; the full spec and the `cycle::run` driver arrive in L2).
//! Every activity reads the same arguments the same way: an optional
//! count, an optional explicit resource (melee spots), and the `toll` /
//! `wild` crossing flags. Old CLIs (`run woodcut 3 toll wild`) keep
//! working through this parser (D25).

#![forbid(unsafe_code)]

use crate::ScriptError;

/// Minimal task specification (L2a): what the brain will fix in L2
/// (activity, exact resource, objectives, budget, keep-list, flags).
/// The old `Result<i32>` contract is untouched.
pub struct TaskSpec {
    /// Activity name, as in the registry (`woodcut`, `mining`…).
    pub activity: &'static str,
    /// Explicit resource (melee spots; empty when the skill picks its
    /// own tier by level, as the brain will fix it in L2).
    pub resource: Vec<String>,
    /// Count target (loads, kills; 0 = unlimited, as before).
    pub count: u32,
    /// Tick budget (melee `--ticks`; 0 = unlimited, as before).
    pub ticks: u64,
    /// Skip the bank loop (melee `--no-bank`).
    pub no_bank: bool,
    /// Paid crossings allowed (`toll`).
    pub allow_paid: bool,
    /// Hunter zones allowed (`wild`, woodcut only today).
    pub allow_wild: bool,
}

/// Parse the legacy CLI into a `TaskSpec`. Same semantics as the old
/// per-script parsing, in one place.
pub fn parse_cli(activity: &'static str, args: &[String]) -> Result<TaskSpec, ScriptError> {
    let usage = || ScriptError::Usage(format!("{activity} [count|spot...] [toll] [wild] [--kills N] [--ticks N] [--no-bank]"));
    let mut spec = TaskSpec {
        activity,
        resource: Vec::new(),
        count: 0,
        ticks: 0,
        no_bank: false,
        allow_paid: false,
        allow_wild: false,
    };
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "toll" => {
                spec.allow_paid = true;
                index += 1;
            }
            "wild" => {
                spec.allow_wild = true;
                index += 1;
            }
            "--no-bank" => {
                spec.no_bank = true;
                index += 1;
            }
            "--kills" | "--ticks" => {
                let value = args
                    .get(index + 1)
                    .and_then(|arg| arg.parse::<u64>().ok())
                    .ok_or_else(usage)?;
                if args[index] == "--kills" {
                    spec.count = value.min(u32::MAX as u64) as u32;
                } else {
                    spec.ticks = value;
                }
                index += 2;
            }
            other => {
                if activity == "melee" {
                    spec.resource.push(other.to_string());
                } else if spec.count == 0 {
                    spec.count = other.parse().map_err(|_| usage())?;
                } else {
                    return Err(usage());
                }
                index += 1;
            }
        }
    }
    Ok(spec)
}

#[cfg(test)]
mod tests {
    use super::parse_cli;

    /// Legacy CLIs parse identically through the common parser.
    #[test]
    fn legacy_clis_parse_to_the_same_spec() {
        let woodcut = parse_cli("woodcut", &["3".to_string(), "toll".to_string(), "wild".to_string()]).expect("woodcut parses");
        assert_eq!((woodcut.count, woodcut.allow_paid, woodcut.allow_wild), (3, true, true));
        assert!(woodcut.resource.is_empty(), "woodcut picks its tier by level");
        let mining = parse_cli("mining", &["1".to_string()]).expect("mining parses");
        assert_eq!((mining.count, mining.allow_paid), (1, false));
        let melee = parse_cli(
            "melee",
            &["goblins".to_string(), "--kills".to_string(), "6".to_string(), "--no-bank".to_string()],
        )
        .expect("melee parses");
        assert_eq!(melee.resource, vec!["goblins".to_string()], "spot is the explicit resource");
        assert_eq!((melee.count, melee.ticks), (6, 0));
        assert!(melee.no_bank, "no-bank kept");
        assert!(parse_cli("woodcut", &["x".to_string()]).is_err(), "junk count fails loud");
        assert!(parse_cli("melee", &["--kills".to_string()]).is_err(), "dangling flag fails loud");
    }
}
