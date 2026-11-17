#![forbid(unsafe_code)]
//! Gifts to the market makers (`orchestrator fund-makers`): a gift is a
//! DEAL of its own kind riding the whole physical handover of the
//! market (convocation on the Varrock square with departures staggered
//! by ETA, absent party opened, `market deliver` on both workers, double
//! screen trade window, sweep at the deadline, typed failures):
//!
//! - seller = the donor, buyer = the maker, object = coins
//!   (`COINS_ID`), quantity = the amount, price 0: the donor puts the
//!   coins in the window, the maker puts nothing, both accept. Never an
//!   order: no book, no ledger move, no execution, no candle, no print,
//!   no relist; its id (`GIFT_ID_BASE` + n) never meets a store row.
//! - the maker's worker checks its pack after "Accepted trade.": the
//!   coins received must equal the amount (`received` in its report,
//!   journaled `received ok` / `received MISMATCH`).
//! - a failed gift (no_show, mismatch = refused, short, timeout,
//!   nav_down) is journaled with its typed reason and created again
//!   once (`RETRIES`); a second failure is `abandoned`, reported in the
//!   daemon log and `orchestrator economy` (section funding).
//! - journal: one JSON line per gift event (`run/funding.jsonl`), plus
//!   one daemon log line each.
//!
//! No cheat (rules 12-13): real trades between fleet accounts, the
//! staff never takes part (`funding_plan`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use lifescape_json::{object, str_value, Value};
use lifescape_market::{Deal, DealStatus};

use crate::market_service::MarketService;

/// Gift deal ids start here: far above every store deal id.
pub const GIFT_ID_BASE: u64 = 1_000_000_000_000;
/// Object id of coins (the gift's goods).
pub const COINS_ID: i64 = 995;
/// A failed gift is created again this many times.
pub const RETRIES: u32 = 1;

/// One gift's terms and progress.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GiftMeta {
    pub donor: String,
    pub maker: String,
    pub amount: i64,
    /// 1 for the first try, 2 for the retry.
    pub attempt: u32,
    /// Coins the maker's pack gained at the window (its report).
    pub received: Option<i64>,
    /// Terminal outcome already handled (journal, retry).
    pub closed: bool,
}

/// The market's gifts (in memory: a daemon restart drops the ones in
/// flight, the journal shows them `lost`, `fund-makers` plans again).
#[derive(Debug, Default)]
pub struct GiftBook {
    next: u64,
    pub(crate) gifts: BTreeMap<u64, GiftMeta>,
    /// Journal file; `None` (tests) journals nothing.
    pub(crate) journal: Option<PathBuf>,
}

/// The daemon's journal file.
pub fn journal_path() -> PathBuf {
    crate::paths::run_dir().join("funding.jsonl")
}

/// This daemon run's stamp (first call, unix seconds).
fn boot_stamp() -> i64 {
    static BOOT: std::sync::OnceLock<i64> = std::sync::OnceLock::new();
    *BOOT.get_or_init(MarketService::now_clock)
}

/// True for a gift deal id.
pub fn is_gift_id(deal_id: u64) -> bool {
    deal_id > GIFT_ID_BASE
}

fn journal_line(id: u64, meta: &GiftMeta, event: &str, reason: &str) -> Value {
    object(vec![
        ("t", Value::Int(MarketService::now_clock())),
        ("boot", Value::Int(boot_stamp())),
        ("gift", Value::Int(id as i64)),
        ("donor", str_value(&meta.donor)),
        ("maker", str_value(&meta.maker)),
        ("amount", Value::Int(meta.amount)),
        ("attempt", Value::Int(i64::from(meta.attempt))),
        ("event", str_value(event)),
        ("reason", str_value(reason)),
        ("received", meta.received.map(Value::Int).unwrap_or(Value::Null)),
    ])
}

fn append(path: Option<&Path>, line: &Value) {
    let Some(path) = path else { return };
    use std::io::Write;
    let written = std::fs::OpenOptions::new().create(true).append(true).open(path).and_then(|mut file| writeln!(file, "{}", lifescape_json::stringify(line)));
    if let Err(error) = written {
        crate::logln!("funding: journal {} not written: {error}", path.display());
    }
}

impl MarketService {
    /// Journals the funding events to `path` (the daemon, at start).
    pub(crate) fn set_funding_journal(&self, path: PathBuf) {
        self.lock_engine().gifts.journal = Some(path);
    }

    fn journal(&self, id: u64, meta: &GiftMeta, event: &str, reason: &str) {
        let path = self.lock_engine().gifts.journal.clone();
        append(path.as_deref(), &journal_line(id, meta, event, reason));
    }

    /// One gift deal, `matched`: the convene loop convenes it like any
    /// deal (grouping window, staggered departures, absent party opened).
    pub(crate) fn create_gift(&self, donor: &str, maker: &str, amount: i64, attempt: u32) -> Result<u64, String> {
        if donor.is_empty() || maker.is_empty() || donor == maker {
            return Err(format!("bad gift parties {donor:?} -> {maker:?}"));
        }
        if amount <= 0 || amount > i64::from(i32::MAX) {
            return Err(format!("bad gift amount {amount}"));
        }
        let clock = Self::now_clock();
        let meta = GiftMeta { donor: donor.to_string(), maker: maker.to_string(), amount, attempt, received: None, closed: false };
        let id = {
            let mut engine = self.lock_engine();
            engine.gifts.next += 1;
            let id = GIFT_ID_BASE + engine.gifts.next;
            let (meeting_x, meeting_y) = crate::market_service::RING[engine.deals.len() % crate::market_service::RING.len()];
            let deal = Deal {
                id,
                buyer: maker.to_string(),
                seller: donor.to_string(),
                item: COINS_ID,
                quantity: amount,
                price: 0,
                meeting_x,
                meeting_y,
                deadline_tick: clock + 3600,
                status: DealStatus::Matched,
                fail: None,
            };
            engine.deals.insert(id, deal);
            engine.arrived.insert(id, (false, false));
            engine.gifts.gifts.insert(id, meta.clone());
            id
        };
        self.journal(id, &meta, "created", "");
        crate::logln!("funding: gift {id} {donor} -> {maker} : {amount} coins (attempt {attempt}), convened at the next round");
        Ok(id)
    }

    /// True for a gift this market holds.
    pub(crate) fn is_gift(&self, deal_id: u64) -> bool {
        is_gift_id(deal_id) && self.lock_engine().gifts.gifts.contains_key(&deal_id)
    }

    /// Gifts not settled nor failed yet: (donor, maker, amount).
    pub(crate) fn gifts_in_flight(&self) -> Vec<(String, String, i64)> {
        let engine = self.lock_engine();
        engine
            .gifts
            .gifts
            .iter()
            .filter(|(id, _)| engine.deals.get(id).is_some_and(|deal| !matches!(deal.status, DealStatus::Settled | DealStatus::Failed)))
            .map(|(_, meta)| (meta.donor.clone(), meta.maker.clone(), meta.amount))
            .collect()
    }

    /// Coins `account` still owes the makers (gifts in flight it gives):
    /// committed like a buyer deal's coins (`brain_gear::committed_coins`).
    pub(crate) fn gifts_owed(&self, account: &str) -> i64 {
        self.gifts_in_flight().iter().filter(|(donor, _, _)| donor == account).map(|(_, _, amount)| *amount).sum()
    }

    /// The account's open gifts in the `my_deals` row shape: the worker
    /// delivers them like its deals.
    pub(crate) fn gift_rows(&self, account: &str) -> Vec<Value> {
        let engine = self.lock_engine();
        engine
            .gifts
            .gifts
            .keys()
            .filter_map(|id| engine.deals.get(id))
            .filter(|deal| (deal.buyer == account || deal.seller == account) && !matches!(deal.status, DealStatus::Settled | DealStatus::Failed))
            .map(|deal| {
                object(vec![
                    ("deal", Value::Int(deal.id as i64)),
                    ("buyer", str_value(&deal.buyer)),
                    ("seller", str_value(&deal.seller)),
                    ("item", Value::Int(deal.item)),
                    ("qty", Value::Int(deal.quantity)),
                    ("price", Value::Int(0)),
                    ("x", Value::Int(deal.meeting_x)),
                    ("y", Value::Int(deal.meeting_y)),
                    ("status", str_value(deal.status.as_str())),
                    ("reason", str_value("")),
                    ("deadline", Value::Int(deal.deadline_tick)),
                    ("gift", Value::Bool(true)),
                ])
            })
            .collect()
    }

    /// The maker's count of coins its pack gained at the window.
    pub(crate) fn gift_received(&self, deal_id: u64, account: &str, received: i64) {
        let meta = {
            let mut engine = self.lock_engine();
            let Some(meta) = engine.gifts.gifts.get_mut(&deal_id) else { return };
            if meta.maker != account {
                return;
            }
            meta.received = Some(received);
            meta.clone()
        };
        let verdict = if received == meta.amount { "received ok" } else { "received MISMATCH" };
        self.journal(deal_id, &meta, "received", verdict);
        crate::logln!("funding: gift {deal_id} {} -> {}: {verdict} ({received} of {} coins)", meta.donor, meta.maker, meta.amount);
    }

    /// A gift reached `settled` or `failed` (report, sweep, absent
    /// party): convene state freed, journaled once; a failure is created
    /// again once, then abandoned.
    pub(crate) fn gift_ended(&self, deal_id: u64) {
        self.release_convene(deal_id);
        let ended = {
            let mut engine = self.lock_engine();
            let outcome = engine.deals.get(&deal_id).map(|deal| (deal.status, deal.fail));
            match (engine.gifts.gifts.get_mut(&deal_id), outcome) {
                (Some(meta), Some((status, fail))) if !meta.closed && matches!(status, DealStatus::Settled | DealStatus::Failed) => {
                    meta.closed = true;
                    Some((meta.clone(), status, fail))
                }
                _ => None,
            }
        };
        let Some((meta, status, fail)) = ended else { return };
        if status == DealStatus::Settled {
            self.journal(deal_id, &meta, "settled", "");
            crate::logln!("funding: gift {deal_id} {} -> {} : {} coins settled", meta.donor, meta.maker, meta.amount);
            return;
        }
        let reason = fail.map(|why| why.as_str()).unwrap_or("timeout");
        self.journal(deal_id, &meta, "failed", reason);
        if meta.attempt <= RETRIES {
            match self.create_gift(&meta.donor, &meta.maker, meta.amount, meta.attempt + 1) {
                Ok(retry) => {
                    self.journal(deal_id, &meta, "retry", &format!("gift {retry}"));
                    crate::logln!("funding: gift {deal_id} {} -> {} failed {reason}, retried once as gift {retry}", meta.donor, meta.maker);
                }
                Err(error) => {
                    self.journal(deal_id, &meta, "abandoned", &format!("{reason}; retry refused: {error}"));
                    crate::logln!("funding: gift {deal_id} {} -> {} failed {reason}, retry refused: {error}", meta.donor, meta.maker);
                }
            }
            return;
        }
        self.journal(deal_id, &meta, "abandoned", reason);
        crate::logln!("funding: gift {deal_id} {} -> {} : {} coins ABANDONED after {} tries (last: {reason})", meta.donor, meta.maker, meta.amount, meta.attempt);
    }
}

/// One gift as the journal tells it (the last word on it).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GiftSummary {
    pub boot: i64,
    pub gift: i64,
    pub donor: String,
    pub maker: String,
    pub amount: i64,
    pub attempt: i64,
    /// `in flight`, `settled`, `failed <reason>`, `abandoned <reason>`,
    /// `lost (daemon restart)`.
    pub state: String,
    pub received: Option<i64>,
}

/// Every gift of the journal, oldest first. A gift without a terminal
/// line from an older daemon run than the last one is `lost`.
pub fn summarize(text: &str) -> Vec<GiftSummary> {
    let mut gifts: BTreeMap<(i64, i64), GiftSummary> = BTreeMap::new();
    let mut last_boot = 0;
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let Ok(value) = lifescape_json::parse(line) else { continue };
        let boot = value.get("boot").as_i64().unwrap_or(0);
        last_boot = last_boot.max(boot);
        let gift = value.get("gift").as_i64().unwrap_or(0);
        let entry = gifts.entry((boot, gift)).or_insert_with(|| GiftSummary {
            boot,
            gift,
            donor: value.get("donor").as_str().unwrap_or("").to_string(),
            maker: value.get("maker").as_str().unwrap_or("").to_string(),
            amount: value.get("amount").as_i64().unwrap_or(0),
            attempt: value.get("attempt").as_i64().unwrap_or(1),
            state: "in flight".to_string(),
            received: None,
        });
        if let Some(received) = value.get("received").as_i64() {
            entry.received = Some(received);
        }
        let reason = value.get("reason").as_str().unwrap_or("");
        match value.get("event").as_str().unwrap_or("") {
            "settled" => entry.state = "settled".to_string(),
            "failed" => entry.state = format!("failed {reason}"),
            "retry" => entry.state = format!("{}, retried as {reason}", entry.state),
            "abandoned" => entry.state = format!("abandoned {reason}"),
            _ => {}
        }
    }
    let mut out: Vec<GiftSummary> = gifts.into_values().collect();
    for summary in &mut out {
        if summary.state == "in flight" && summary.boot < last_boot {
            summary.state = "lost (daemon restart)".to_string();
        }
    }
    out
}

/// The `funding` section of `orchestrator economy`.
pub fn section_lines(summaries: &[GiftSummary]) -> Vec<String> {
    let mut lines = vec![format!("funding (fund-makers gifts, journal {}):", journal_path().display())];
    if summaries.is_empty() {
        lines.push("  no gift yet".to_string());
        return lines;
    }
    let settled: Vec<&GiftSummary> = summaries.iter().filter(|gift| gift.state == "settled").collect();
    let moved: i64 = settled.iter().map(|gift| gift.received.unwrap_or(gift.amount)).sum();
    let failed = summaries.iter().filter(|gift| gift.state.starts_with("failed")).count();
    let abandoned = summaries.iter().filter(|gift| gift.state.starts_with("abandoned")).count();
    let flying = summaries.iter().filter(|gift| gift.state == "in flight").count();
    lines.push(format!("  {} gifts: {} settled ({moved} coins), {flying} in flight, {failed} failed (retried), {abandoned} abandoned", summaries.len(), settled.len()));
    for gift in summaries {
        let check = match gift.received {
            Some(received) if received == gift.amount => " (received ok)".to_string(),
            Some(received) => format!(" (received {received}: MISMATCH)"),
            None => String::new(),
        };
        lines.push(format!("  gift {} try {}: {} -> {} : {} coins, {}{check}", gift.gift, gift.attempt, gift.donor, gift.maker, gift.amount, gift.state));
    }
    lines
}

#[cfg(test)]
#[path = "funding_gifts_tests.rs"]
mod tests;
