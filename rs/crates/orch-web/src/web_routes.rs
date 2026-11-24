//! The GET route table (lot 5.7, 2026-11-24): `handle_get` was one 270-line
//! chain of `if parts == [...]`. Here every route is a row, a matcher and
//! a handler, in one table, and the router tries the rows in order.
//!
//! Same routes, same answers (status, content type, cache header, body):
//! `web_routes_tests` pins them. No two rows match one path, so the order
//! is only a reading order: pages first, then the API.

use lifescape_json::{self as json, object, str_value, Value};

use crate::paths;
use crate::web::{ok_json, parse_query, Dispatch, Response};

/// What a handler reads from the request.
pub(crate) struct Ctx<'a> {
    pub(crate) parts: &'a [&'a str],
    pub(crate) query: &'a str,
    pub(crate) dispatch: &'a Dispatch,
}

type Handler = fn(&Ctx) -> Result<Response, String>;

struct Route {
    matches: fn(&[&str]) -> bool,
    handle: Handler,
}

/// One static file served as is: the path, its file in `orchestration/static`
/// and its content type.
struct Page {
    path: &'static str,
    file: &'static str,
    content_type: &'static str,
}

const HTML: &str = "text/html; charset=utf-8";
const JS: &str = "application/javascript";

/// Every page and script (re-read from disk on every request).
const PAGES: &[Page] = &[
    Page { path: "", file: "index.html", content_type: HTML },
    Page { path: "spectator.js", file: "spectator.js", content_type: JS },
    Page { path: "items", file: "items.html", content_type: HTML },
    Page { path: "items.js", file: "items.js", content_type: JS },
    Page { path: "market", file: "market.html", content_type: HTML },
    Page { path: "market.js", file: "market.js", content_type: JS },
    Page { path: "chart.js", file: "chart.js", content_type: JS },
    Page { path: "table.js", file: "table.js", content_type: JS },
    Page { path: "rates", file: "rates.html", content_type: HTML },
    Page { path: "rates.js", file: "rates.js", content_type: JS },
    Page { path: "brain.js", file: "brain.js", content_type: JS },
    Page { path: "hiscores", file: "hiscores.html", content_type: HTML },
    Page { path: "hiscores.js", file: "hiscores.js", content_type: JS },
    Page { path: "xp", file: "xp.html", content_type: HTML },
    Page { path: "xp.js", file: "xp.js", content_type: JS },
    Page { path: "life", file: "life.html", content_type: HTML },
    Page { path: "life.js", file: "life.js", content_type: JS },
    Page { path: "economy", file: "economy.html", content_type: HTML },
    Page { path: "economy.js", file: "economy.js", content_type: JS },
    Page { path: "guides-money", file: "guides_money.html", content_type: HTML },
    Page { path: "guides-training", file: "guides_training.html", content_type: HTML },
    Page { path: "guides.js", file: "guides.js", content_type: JS },
    Page { path: "icons.js", file: "icons.js", content_type: JS },
];

/// The page of a path (`[]` is the index, a single segment otherwise).
fn page_of(parts: &[&str]) -> Option<&'static Page> {
    let path = match parts {
        [] => "",
        [one] => one,
        _ => return None,
    };
    PAGES.iter().find(|page| page.path == path)
}

fn asset(name: &str) -> std::io::Result<Vec<u8>> {
    std::fs::read(paths::static_dir().join(name))
}

fn serve_page(ctx: &Ctx) -> Result<Response, String> {
    let page = page_of(ctx.parts).ok_or("no such page")?;
    Ok(Response { status: 200, content_type: page.content_type, cache: "no-store", body: asset(page.file).map_err(|error| error.to_string())? })
}

/// One precomputed map file: (dir, file) when the path is exactly
/// `map-<16 hex>/<name>.png|manifest.json`, nothing otherwise (no
/// traversal: parts are already split on '/', ".." never matches).
pub(crate) fn map_asset(parts: &[&str]) -> Option<(String, String)> {
    if parts.len() != 2 {
        return None;
    }
    let (dir, file) = (parts[0], parts[1]);
    if dir.len() != 20 || !dir.starts_with("map-") || !dir[4..].chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let ok = file == "manifest.json"
        || (file.ends_with(".png")
            && file.len() <= 64
            && file.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.'));
    if !ok {
        return None;
    }
    Some((dir.to_string(), file.to_string()))
}

fn serve_map_file(ctx: &Ctx) -> Result<Response, String> {
    let (dir, file) = map_asset(ctx.parts).ok_or("no such map file")?;
    let body = std::fs::read(paths::static_dir().join(&dir).join(&file)).map_err(|error| error.to_string())?;
    Ok(Response {
        status: 200,
        content_type: if file.ends_with(".json") { "application/json" } else { "image/png" },
        cache: "public, max-age=31536000, immutable",
        body,
    })
}

/// One icon-sheet file (2026-11-24): `icons/index.json` or
/// `icons/<name>.png`, written by `gamedata/tools/icons.mts` into
/// `orchestration/static/icons/`. Nothing else (no traversal: ".." and
/// any other name never match).
pub(crate) fn icon_asset<'a>(parts: &[&'a str]) -> Option<&'a str> {
    if parts.len() != 2 || parts[0] != "icons" {
        return None;
    }
    let file = parts[1];
    let png = file.len() > 4
        && file.len() <= 32
        && file.ends_with(".png")
        && file[..file.len() - 4].chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    (file == "index.json" || png).then_some(file)
}

/// The index changes when the sheets are regenerated: never cached. The
/// sheets are fetched as `<name>.png?v=<hash of the bytes>` (the hash
/// comes from the index), so a long cache is safe.
fn serve_icon_file(ctx: &Ctx) -> Result<Response, String> {
    let file = icon_asset(ctx.parts).ok_or("no such icon file")?;
    let body = match std::fs::read(paths::static_dir().join("icons").join(file)) {
        Ok(body) => body,
        Err(_) => return Ok(crate::web::not_found()),
    };
    Ok(if file == "index.json" {
        Response { status: 200, content_type: "application/json", cache: "no-store", body }
    } else {
        Response { status: 200, content_type: "image/png", cache: "public, max-age=31536000, immutable", body }
    })
}

fn is_mapscene(parts: &[&str]) -> bool {
    parts.len() == 2 && parts[0] == "mapscene" && parts[1].ends_with(".png")
}

fn serve_mapscene(ctx: &Ctx) -> Result<Response, String> {
    let name = ctx.parts[1];
    let png = name[..name.len() - 4].parse::<i32>().ok().and_then(|sprite| crate::web_ws::shared_maps().cache().mapscene_png(sprite));
    match png {
        Some(body) => Ok(Response { status: 200, content_type: "image/png", cache: "public, max-age=86400", body }),
        None => Ok(crate::web::not_found()),
    }
}

/// `/api/<area>[/<name>]`: the name segment, empty when absent.
fn name_of<'a>(parts: &[&'a str]) -> &'a str {
    if parts.len() > 2 {
        parts[2]
    } else {
        ""
    }
}

fn under(parts: &[&str], prefix: [&str; 2]) -> bool {
    parts.len() >= 2 && parts[..2] == prefix
}

fn api_status(ctx: &Ctx) -> Result<Response, String> {
    Ok(ok_json((ctx.dispatch)(object(vec![("cmd", str_value("status"))]))))
}

fn api_map(_: &Ctx) -> Result<Response, String> {
    let pointer = std::fs::read_to_string(paths::static_dir().join("map-current.json")).map_err(|error| error.to_string())?;
    let parsed = json::parse(&pointer).map_err(|error| error.to_string())?;
    Ok(ok_json(object(vec![("ok", Value::Bool(true)), ("dir", parsed.get("dir").clone()), ("hash", parsed.get("hash").clone())])))
}

fn api_state(ctx: &Ctx) -> Result<Response, String> {
    let reply = (ctx.dispatch)(object(vec![("cmd", str_value("show")), ("account", str_value(name_of(ctx.parts)))]));
    let state = reply.get("state").clone();
    let mut shaped = std::collections::BTreeMap::new();
    shaped.insert("ok".to_string(), reply.get("ok").clone());
    shaped.insert("state".to_string(), crate::web_ws::annotate_mapscene(state));
    Ok(ok_json(Value::Obj(shaped)))
}

fn api_rates_table(ctx: &Ctx) -> Result<Response, String> {
    Ok(ok_json(crate::rates_service::rates_api(&**ctx.dispatch)))
}

fn api_guides(ctx: &Ctx) -> Result<Response, String> {
    let kind = if ctx.parts[2] == "money" { crate::guides_model::GuideKind::Money } else { crate::guides_model::GuideKind::Training };
    Ok(ok_json(crate::web_guides::guides_api(kind, ctx.query)))
}

fn api_lives(_: &Ctx) -> Result<Response, String> {
    Ok(ok_json(crate::web_lives::lives_response()))
}

fn api_life(ctx: &Ctx) -> Result<Response, String> {
    Ok(ok_json(crate::web_lives::life_response(name_of(ctx.parts))))
}

fn api_brain(ctx: &Ctx) -> Result<Response, String> {
    Ok(ok_json(crate::autopilot::current().brain_view(name_of(ctx.parts), ctx.dispatch)))
}

/// Cached snapshot bytes only: no computation per request.
fn api_xp(_: &Ctx) -> Result<Response, String> {
    Ok(Response { status: 200, content_type: "application/json", cache: "no-store", body: crate::xp_service::api_body() })
}

fn api_hiscores(_: &Ctx) -> Result<Response, String> {
    Ok(Response { status: 200, content_type: "application/json", cache: "no-store", body: crate::hiscores_service::api_body() })
}

fn api_economy(ctx: &Ctx) -> Result<Response, String> {
    Ok(ok_json(crate::web_lives::economy_response(ctx.dispatch)))
}

fn api_market(ctx: &Ctx) -> Result<Response, String> {
    Ok(crate::web_market::market_api(ctx.parts[2], ctx.query, ctx.dispatch))
}

fn api_items_history(ctx: &Ctx) -> Result<Response, String> {
    Ok(crate::web_items::items_history_api(ctx.query, &paths::run_dir()))
}

fn api_items_flows(ctx: &Ctx) -> Result<Response, String> {
    Ok(crate::web_items::items_flows_api(ctx.query, &paths::run_dir()))
}

fn api_items(ctx: &Ctx) -> Result<Response, String> {
    Ok(crate::web_items::items_list_api(ctx.dispatch))
}

fn api_scripts(ctx: &Ctx) -> Result<Response, String> {
    Ok(ok_json((ctx.dispatch)(object(vec![("cmd", str_value("scripts"))]))))
}

fn api_accounts(ctx: &Ctx) -> Result<Response, String> {
    Ok(ok_json((ctx.dispatch)(object(vec![("cmd", str_value("accounts"))]))))
}

fn api_logs(ctx: &Ctx) -> Result<Response, String> {
    let lines = parse_query(ctx.query).get("lines").and_then(|raw| raw.parse::<i64>().ok()).unwrap_or(60);
    Ok(ok_json((ctx.dispatch)(object(vec![
        ("cmd", str_value("logs")),
        ("account", str_value(name_of(ctx.parts))),
        ("lines", Value::Int(lines)),
    ]))))
}

/// The routes that are not a page. Exact paths and `<prefix>[/<name>]`.
const ROUTES: &[Route] = &[
    Route { matches: |parts| page_of(parts).is_some(), handle: serve_page },
    Route { matches: |parts| map_asset(parts).is_some(), handle: serve_map_file },
    Route { matches: is_mapscene, handle: serve_mapscene },
    Route { matches: |parts| icon_asset(parts).is_some(), handle: serve_icon_file },
    Route { matches: |parts| parts == ["api", "status"], handle: api_status },
    Route { matches: |parts| parts == ["api", "map"], handle: api_map },
    Route { matches: |parts| under(parts, ["api", "state"]), handle: api_state },
    Route { matches: |parts| parts == ["api", "rates", "table"], handle: api_rates_table },
    Route { matches: |parts| parts == ["api", "guides", "money"] || parts == ["api", "guides", "training"], handle: api_guides },
    Route { matches: |parts| parts == ["api", "lives"], handle: api_lives },
    Route { matches: |parts| under(parts, ["api", "life"]), handle: api_life },
    Route { matches: |parts| under(parts, ["api", "brain"]), handle: api_brain },
    Route { matches: |parts| parts == ["api", "xp"], handle: api_xp },
    Route { matches: |parts| parts == ["api", "hiscores"], handle: api_hiscores },
    Route { matches: |parts| parts == ["api", "economy"], handle: api_economy },
    Route { matches: |parts| parts.len() >= 3 && parts[..2] == ["api", "market"], handle: api_market },
    Route { matches: |parts| parts == ["api", "items", "history"], handle: api_items_history },
    Route { matches: |parts| parts == ["api", "items", "flows"], handle: api_items_flows },
    Route { matches: |parts| parts == ["api", "items"], handle: api_items },
    Route { matches: |parts| parts == ["api", "scripts"], handle: api_scripts },
    Route { matches: |parts| parts == ["api", "accounts"], handle: api_accounts },
    Route { matches: |parts| under(parts, ["api", "logs"]), handle: api_logs },
];

/// The route of a request: the table, then the pages that own their
/// own sub-router (`errors`, `rates` views), else not found.
pub(crate) fn route(parts: &[&str], query: &str, dispatch: &Dispatch) -> Result<Response, String> {
    let ctx = Ctx { parts, query, dispatch };
    if let Some(found) = ROUTES.iter().find(|route| (route.matches)(parts)) {
        return (found.handle)(&ctx);
    }
    if let Some(reply) = crate::web_errors::route(parts, query) {
        return Ok(reply);
    }
    if let Some(reply) = crate::web_rates_view::route(parts, query, &**dispatch) {
        return Ok(reply);
    }
    Ok(crate::web::not_found())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_asset_accepts_only_hashed_files() {
        assert_eq!(
            map_asset(&["map-0123456789abcdef", "c44-l0.png"]),
            Some(("map-0123456789abcdef".to_string(), "c44-l0.png".to_string()))
        );
        assert_eq!(
            map_asset(&["map-0123456789abcdef", "manifest.json"]),
            Some(("map-0123456789abcdef".to_string(), "manifest.json".to_string()))
        );
        assert_eq!(map_asset(&["map-xyz", "c44-l0.png"]), None);
        assert_eq!(map_asset(&["map-0123456789abcdef", "x.png", "y"]), None);
        assert_eq!(map_asset(&["map-0123456789abcdef", ".."]), None);
        assert_eq!(map_asset(&["map-0123456789abcdef", "c44-l0.jpg"]), None);
        assert_eq!(map_asset(&["map-0123456789ABCDEF", "c44-l0.png"]), Some(("map-0123456789ABCDEF".to_string(), "c44-l0.png".to_string())));
        assert_eq!(map_asset(&["map-0123456789abcde", "c44-l0.png"]), None);
    }

    #[test]
    fn icon_asset_accepts_only_the_sheets_and_the_index() {
        assert_eq!(icon_asset(&["icons", "index.json"]), Some("index.json"));
        assert_eq!(icon_asset(&["icons", "items.png"]), Some("items.png"));
        assert_eq!(icon_asset(&["icons", "skills.png"]), Some("skills.png"));
        assert_eq!(icon_asset(&["icons", ".png"]), None);
        assert_eq!(icon_asset(&["icons", ".."]), None);
        assert_eq!(icon_asset(&["icons", "a.b.png"]), None);
        assert_eq!(icon_asset(&["icons", "other.json"]), None);
        assert_eq!(icon_asset(&["icons", "items.png", "x"]), None);
        assert_eq!(icon_asset(&["icon", "items.png"]), None);
        assert_eq!(icon_asset(&["icons"]), None);
    }

    /// No two rows claim one path: the table order is a reading order,
    /// never a precedence (the two sub-routers, `errors` and the rates
    /// views, own paths no row has).
    #[test]
    fn no_two_routes_claim_the_same_path() {
        let probes: &[&[&str]] = &[
            &[],
            &["items"],
            &["errors"],
            &["api", "status"],
            &["api", "state", "x"],
            &["api", "items"],
            &["api", "items", "history"],
            &["api", "market", "trades"],
            &["api", "rates", "table"],
            &["api", "rates", "detail"],
            &["api", "guides", "money"],
            &["map-0123456789abcdef", "manifest.json"],
            &["mapscene", "1.png"],
            &["icons", "items.png"],
            &["icons", "index.json"],
            &["icons.js"],
        ];
        for parts in probes {
            let rows = ROUTES.iter().filter(|route| (route.matches)(parts)).count();
            assert!(rows <= 1, "{parts:?} matched {rows} rows");
        }
    }
}
