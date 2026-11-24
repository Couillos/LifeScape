//! Characterisation of the GET routes (lot 5.7, 2026-11-24): the route
//! table must answer every path exactly as the `if` chain did (status,
//! content type, cache header, body). One digest over a list of paths:
//! static pages, the dispatch-backed APIs, prefixes with and without a
//! name, map assets, and misses.

use lifescape_json::{self as json, Value};

fn fnv(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// Paths whose answers depend on nothing but the repository and the stub.
const PATHS: &[(&str, &str)] = &[
    ("/", ""),
    ("/spectator.js", ""),
    ("/items", ""),
    ("/items.js", ""),
    ("/market", ""),
    ("/market.js", ""),
    ("/chart.js", ""),
    ("/table.js", ""),
    ("/rates", ""),
    ("/rates.js", ""),
    ("/brain.js", ""),
    ("/hiscores", ""),
    ("/hiscores.js", ""),
    ("/xp", ""),
    ("/xp.js", ""),
    ("/life", ""),
    ("/life.js", ""),
    ("/economy", ""),
    ("/economy.js", ""),
    ("/errors", ""),
    ("/errors.js", ""),
    ("/guides-money", ""),
    ("/guides-training", ""),
    ("/guides.js", ""),
    ("/api/status", ""),
    ("/api/map", ""),
    ("/api/state/smith", ""),
    ("/api/state", ""),
    ("/api/scripts", ""),
    ("/api/accounts", ""),
    ("/api/logs/smith", "lines=5"),
    ("/api/logs/smith", "lines=x"),
    ("/api/logs", ""),
    ("/mapscene/abc.png", ""),
    ("/mapscene/12.jpg", ""),
    ("/map-8762ed9c85f8b53a/manifest.json", ""),
    ("/map-8762ed9c85f8b53a/missing.png", ""),
    ("/map-xyz/manifest.json", ""),
    ("/nope", ""),
    ("/api/nope", ""),
    ("/api", ""),
    ("/a/b/c", ""),
    ("//items//", ""),
    ("/items/extra", ""),
    ("/api/market", ""),
    ("/api/items/unknown", ""),
];

/// The pages and scripts, with the file of `orchestration/static` each one
/// serves (written from the old chain, not from the table): the body is
/// compared with the file on disk, so editing a page never breaks the pin.
const PAGE_FILES: &[(&str, &str)] = &[
    ("/", "index.html"),
    ("/spectator.js", "spectator.js"),
    ("/items", "items.html"),
    ("/items.js", "items.js"),
    ("/market", "market.html"),
    ("/market.js", "market.js"),
    ("/chart.js", "chart.js"),
    ("/table.js", "table.js"),
    ("/rates", "rates.html"),
    ("/rates.js", "rates.js"),
    ("/brain.js", "brain.js"),
    ("/hiscores", "hiscores.html"),
    ("/hiscores.js", "hiscores.js"),
    ("/xp", "xp.html"),
    ("/xp.js", "xp.js"),
    ("/life", "life.html"),
    ("/life.js", "life.js"),
    ("/economy", "economy.html"),
    ("/economy.js", "economy.js"),
    ("/errors", "errors.html"),
    ("/errors.js", "errors.js"),
    ("/guides-money", "guides_money.html"),
    ("/guides-training", "guides_training.html"),
    ("/guides.js", "guides.js"),
    ("//items//", "items.html"),
];

fn digest() -> String {
    let dispatch: crate::web::Dispatch = std::sync::Arc::new(|request: Value| {
        json::object(vec![("ok", Value::Bool(true)), ("echo", Value::Str(json::stringify(&request)))])
    });
    let mut lines = Vec::new();
    for (path, query) in PATHS {
        let reply = crate::web::handle_get(path, query, &dispatch);
        let body = match PAGE_FILES.iter().find(|(page, _)| page == path) {
            Some((_, file)) => {
                let on_disk = std::fs::read(crate::paths::static_dir().join(file)).expect("the page file exists");
                assert_eq!(reply.body, on_disk, "{path} serves {file}");
                "file".to_string()
            }
            None => format!("{:016x}", fnv(&reply.body)),
        };
        lines.push(format!("{path}?{query} {} {} {} {body}", reply.status, reply.content_type, reply.cache));
    }
    lines.join("\n")
}

#[test]
fn the_route_table_answers_like_the_old_chain() {
    let got = digest();
    let expected = include_str!("web_routes_digest.txt").trim_end();
    assert_eq!(got, expected, "GET answers changed:\n{got}");
}
