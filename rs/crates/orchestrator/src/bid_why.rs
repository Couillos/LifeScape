#![forbid(unsafe_code)]
//! `orchestrator bid-why <account> <object>` around the bid response
//! explanation (`brain_bids::explain_for`): which object the operator
//! named, and where the bids rest when it carries none.
//!
//! C13 (plan-marche §2, 2026-11-16): `bid-why fiona_park stringstar`
//! answered "no bid" while ten bids rested on "Holy symbol". No id was
//! mis-mapped on the bid path: the bids sit on `blessedstar` (id 1718;
//! 1719 is its banknote, same display name, read as the same object by
//! the book view), the Holy symbol Brother Jered blesses out of a
//! `stringstar`, whose display name is "Unblessed symbol". The operator's
//! name pointed at the input, and the answer never said so. Now the
//! command accepts a debug name, a banknote name, a display name or an
//! id, prints which object it read, and when that object carries no bid
//! names the objects made from it (or sharing its display name) that do.

use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::Duration;

use lifescape_planner::market_fit::BookView;

/// Base debug name of a market object named by `raw`: a debug name
/// (`stringstar`), a banknote (`cert_blessedstar`), a display name (any
/// case, `Holy symbol`) or an object id (`1719`). A display name shared
/// by several objects is refused with their debug names.
pub fn resolve_item(raw: &str) -> Result<String, String> {
    let text = raw.trim();
    let base = |name: &str| name.strip_prefix("cert_").unwrap_or(name).to_string();
    if let Ok(id) = text.parse::<usize>() {
        return lifescape_gamedata::obj_by_id(id).and_then(|obj| obj.name).map(base).ok_or_else(|| format!("no object with id {id}"));
    }
    if let Some(name) = lifescape_gamedata::objs_by_name(text).into_iter().find_map(|obj| obj.name) {
        return Ok(base(name));
    }
    let mut shown: Vec<String> = lifescape_gamedata::all_objs()
        .iter()
        .filter(|obj| obj.display.is_some_and(|display| display.eq_ignore_ascii_case(text)))
        .filter_map(|obj| obj.name.map(base))
        .collect();
    shown.sort();
    shown.dedup();
    match shown.as_slice() {
        [only] => Ok(only.clone()),
        [] => Err(format!("unknown object {text:?} (debug name, display name or id)")),
        several => Err(format!("{text:?} names several objects: {}", several.join(", "))),
    }
}

/// `stringstar = "Unblessed symbol" (id 1716, banknote 1717)`.
pub fn identity_line(item: &str) -> String {
    let base = lifescape_gamedata::objs_by_name(item).into_iter().next();
    let note = lifescape_gamedata::objs_by_name(&format!("cert_{item}")).into_iter().next();
    let display = base.and_then(|obj| obj.display).unwrap_or("?");
    let ids = match (base, note) {
        (Some(base), Some(note)) => format!("id {}, banknote {}", base.obj_id, note.obj_id),
        (Some(base), None) => format!("id {}", base.obj_id),
        _ => "no id".to_string(),
    };
    format!("{item} = {display:?} ({ids})")
}

/// Objects a server way makes from each object (`ways(product)` inputs),
/// built once: `stringstar` → `blessedstar` (Brother Jered).
fn consumers() -> &'static HashMap<String, Vec<(String, String)>> {
    static TABLE: OnceLock<HashMap<String, Vec<(String, String)>>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut out: HashMap<String, Vec<(String, String)>> = HashMap::new();
        for obj in lifescape_gamedata::f2p::f2p_objs() {
            let Some(product) = obj.name else { continue };
            if product.starts_with("cert_") {
                continue;
            }
            for way in lifescape_gamedata::obtain::ways(product) {
                if way.kind == lifescape_gamedata::obtain::WayKind::Note {
                    continue;
                }
                for (input, _) in &way.inputs {
                    let made = out.entry(input.clone()).or_default();
                    if !made.iter().any(|(known, _)| known == product) {
                        made.push((product.to_string(), format!("{:?} {}", way.kind, way.detail).to_lowercase()));
                    }
                }
            }
        }
        out
    })
}

/// Objects related to `item` an operator may have meant: made from it,
/// or sharing its display name. `(debug name, how)`.
pub fn related_objects(item: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = consumers().get(item).cloned().unwrap_or_default();
    let display = lifescape_gamedata::objs_by_name(item).into_iter().next().and_then(|obj| obj.display);
    if let Some(display) = display {
        for obj in lifescape_gamedata::all_objs() {
            let Some(name) = obj.name else { continue };
            if name != item && !name.starts_with("cert_") && obj.display == Some(display) && !out.iter().any(|(known, _)| known == name) {
                out.push((name.to_string(), format!("same display name {display:?}")));
            }
        }
    }
    out
}

/// Lines naming where the bids rest when `item` itself carries none:
/// every related object with open bids in `view` (another bot's book).
pub fn related_bid_lines(item: &str, view: &dyn Fn(&str) -> Option<BookView>) -> Vec<String> {
    let mut lines = Vec::new();
    for (other, how) in related_objects(item) {
        let Some(book) = view(&other).filter(|book| !book.bids.is_empty()) else { continue };
        let units: i64 = book.bids.iter().map(|(_, qty)| qty).sum();
        lines.push(format!(
            "  bids rest on {} instead: {} bids, {units} units, best {} ({how}, from {item})",
            identity_line(&other),
            book.bids.len(),
            book.best_bid().unwrap_or(0)
        ));
    }
    lines
}

/// CLI side of `bid-why <account> <object…>` (the daemon answers); the
/// object may be a display name of several words (`Holy symbol`).
pub fn bid_why_cli(argv: &[String]) -> i32 {
    let Some(account) = argv.first().filter(|_| argv.len() >= 2) else {
        eprintln!("usage: bid-why <compte> <objet>");
        return 2;
    };
    let item = argv[1..].join(" ");
    let request = lifescape_json::object(vec![("cmd", lifescape_json::str_value("bid_why")), ("account", lifescape_json::str_value(account)), ("item", lifescape_json::str_value(&item))]);
    match crate::cli::send_command(&request, Duration::from_secs(30)) {
        Ok(reply) if reply.get("ok").as_bool() == Some(true) => {
            for line in reply.get("lines").as_arr().cloned().unwrap_or_default() {
                println!("{}", line.as_str().unwrap_or(""));
            }
            0
        }
        Ok(reply) => {
            eprintln!("bid-why: {}", lifescape_json::stringify(reply.get("error")));
            1
        }
        Err(error) => {
            eprintln!("bid-why: {error}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{identity_line, related_bid_lines, related_objects, resolve_item};
    use lifescape_planner::market_fit::BookView;

    /// C13: every way an operator names the Holy symbol reads the same
    /// object, `blessedstar`; `stringstar` stays the Unblessed symbol.
    #[test]
    fn names_ids_and_displays_resolve_to_the_base_object() {
        for raw in ["blessedstar", "cert_blessedstar", "Holy symbol", "holy symbol", "1718", "1719"] {
            assert_eq!(resolve_item(raw).as_deref(), Ok("blessedstar"), "{raw}");
        }
        assert_eq!(resolve_item("stringstar").as_deref(), Ok("stringstar"));
        assert_eq!(resolve_item("Unblessed symbol").as_deref(), Ok("stringstar"));
        assert!(resolve_item("no such thing").is_err());
        let line = identity_line("stringstar");
        assert!(line.contains("Unblessed symbol") && line.contains("id 1716") && line.contains("banknote 1717"), "{line}");
        assert!(identity_line("blessedstar").contains("\"Holy symbol\" (id 1718, banknote 1719)"));
    }

    /// The live case: ten bids on the Holy symbol, none on the Unblessed
    /// symbol. Asked about `stringstar`, the answer names `blessedstar`,
    /// made from it by Brother Jered.
    #[test]
    fn an_input_without_bids_points_at_the_bid_product() {
        assert!(related_objects("stringstar").iter().any(|(name, how)| name == "blessedstar" && how.contains("brother_jered")), "{:?}", related_objects("stringstar"));
        let view = |name: &str| (name == "blessedstar").then(|| BookView { bids: vec![(530, 1); 10], ..BookView::default() });
        let lines = related_bid_lines("stringstar", &view);
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(lines[0].contains("blessedstar = \"Holy symbol\"") && lines[0].contains("10 bids") && lines[0].contains("best 530"), "{lines:?}");
        assert!(related_bid_lines("stringstar", &|_| None).is_empty(), "no bid anywhere, nothing to add");
    }
}
