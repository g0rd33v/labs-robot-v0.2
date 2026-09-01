//! The Registry (§4b) — the PIMS surface, all five categories.
//!
//! *"Nothing about you exists outside these five categories — and that
//! sentence is checkable, because the categories are the schema, not a
//! summary of it."*
//!
//! [`census`] is that sentence as code. Every table a cell can contain is
//! mapped to a category or named as substrate — the provenance and audit
//! ground the categories point INTO (your own messages are the sources
//! knowledge cites; the journal is the history the receipts cite). The
//! census test walks `sqlite_master` and fails on any table it has never
//! heard of, so a new store cannot be added without answering "which
//! category is this, and what rights apply?" — the exact question that
//! went unasked when `connections` was added, and that this makes
//! unskippable.

use super::{attested, mind_err, note_evidence, Capability, Ctx};
use prism::types::{Effect, Outcome, Rendering};
use prism::PrismError;
use rusqlite::Connection;

/// The five §4b categories, plus the ground they stand on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    Knowledge,
    Instructions,
    Preferences,
    Media,
    Grants,
    /// Not "about you" as a derived conclusion: the raw ground the
    /// categories cite — your own words, the journal, the receipts — plus
    /// engine internals. Governed by the laws directly (provenance,
    /// receipts, boundary log) rather than by category rights.
    Substrate,
}

/// Every table a cell may contain, each answering to a category.
///
/// FTS/vec shadow tables are matched by prefix in [`census`]; everything
/// else must be named here or the census fails.
pub const CELL_TABLES: &[(&str, Category)] = &[
    // knowledge: facts, their graph-to-be, and the ledger of asks
    ("facts", Category::Knowledge),
    ("reminders", Category::Knowledge),
    ("commitments", Category::Knowledge),
    // a contradiction is a statement about two pieces of knowledge, and
    // the Registry surfaces it as "conflicting -- pick one"
    ("fact_contests", Category::Knowledge),
    // instructions
    ("instructions", Category::Instructions),
    // preferences: soul's dial and its history; cell_meta holds settings
    // (lang, soul evolution, quota counters, vec_ready)
    ("soul_persona", Category::Preferences),
    ("soul_revisions", Category::Preferences),
    ("soul_lessons", Category::Preferences),
    ("cell_meta", Category::Preferences),
    // media: the vault and the names over it
    ("media", Category::Media),
    ("files", Category::Media),
    // grants: standing authority to reach the outside
    ("connections", Category::Grants),
    // substrate
    ("messages", Category::Substrate),
    ("journal", Category::Substrate),
    ("receipts", Category::Substrate),
    ("outbox", Category::Substrate),
    ("pending_calls", Category::Substrate),
    // §4.1.6's coalescing window. Holds a HASH of a recent message and the
    // intent that claimed it -- no content, and swept once the two-second
    // window passes. Substrate: it decides whether a message becomes a
    // turn, and remembers nothing about the person once it has.
    ("recent_messages", Category::Substrate),
    ("tombstones", Category::Substrate),
    // soul's own append-only diary of what it changed and why -- audit
    // trail, cited by the preferences rows above
    ("soul_journal", Category::Substrate),
];

/// Shadow/index tables the engine creates around declared ones. An index
/// holds no data of its own -- its content is the declared table's, and its
/// category is that table's category.
fn is_shadow(name: &str) -> bool {
    name.starts_with("facts_fts")
        || name.starts_with("facts_vec")
        || name.starts_with("messages_fts")
        || name == "sqlite_sequence"
}

/// Walk the actual schema; return any table the map cannot account for.
pub fn census(conn: &Connection) -> Result<Vec<String>, rusqlite::Error> {
    let mut stmt = conn
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")?;
    let names = stmt
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(names
        .into_iter()
        .filter(|n| !is_shadow(n) && !CELL_TABLES.iter().any(|(t, _)| t == n))
        .collect())
}

pub struct Show;

impl Capability for Show {
    fn name(&self) -> &'static str {
        "registry.show"
    }
    fn effect(&self) -> Effect {
        Effect::Read
    }
    fn description(&self) -> &'static str {
        "Open the registry: the complete, categorised account of everything \
         the robot holds about the person -- knowledge, standing rules, \
         preferences, media, and connected accounts, with counts. Use when \
         they ask what you know about them overall, what data you hold, or \
         to see the registry."
    }
    fn schema(&self) -> serde_json::Value {
        super::no_args()
    }
    fn validate(&self, _args: &serde_json::Value) -> Result<(), String> {
        Ok(())
    }
    fn execute(&self, ctx: &Ctx<'_>, _args: &serde_json::Value) -> Result<Outcome, PrismError> {
        let overview = ctx.cell.with(|c| overview_json(c).map_err(mind_err))?;
        attested(
            note_evidence("registry.show"),
            "showed the five-category registry".to_string(),
            Rendering::new("registry_overview", overview),
        )
    }
}

/// One JSON shape for the overview and the export: the Registry shown and
/// the Registry exported are the same thing at two levels of detail.
fn overview_json(c: &Connection) -> Result<serde_json::Value, mind::MindError> {
    let count = |sql: &str| -> Result<i64, mind::MindError> {
        Ok(c.query_row(sql, [], |r| r.get(0))?)
    };
    let facts = count("SELECT count(*) FROM facts WHERE status != 'superseded'")?;
    let confirmed = count("SELECT count(*) FROM facts WHERE confirmed_at IS NOT NULL")?;
    let rules = mind::instructions::active(c)?.len();
    let reminders = count("SELECT count(*) FROM reminders WHERE status = 'active'")?;
    let owed = mind::commitments::outstanding(c)?.len();
    let files = mind::files::list(c)?.len();
    let media = count("SELECT count(*) FROM media")?;
    let connected = mind::connections::list(c)?;
    let dial = soul::dial::load(c).ok();
    Ok(serde_json::json!({
        "knowledge": { "facts": facts, "confirmed": confirmed,
                       "reminders": reminders, "commitments_open": owed },
        "instructions": { "active": rules },
        "preferences": { "dial_set": dial.is_some() },
        "media": { "files": files, "vault_objects": media },
        "grants": {
            "accounts": connected.iter()
                .map(|x| serde_json::json!({ "provider": x.provider, "account": x.account }))
                .collect::<Vec<_>>()
        },
    }))
}

pub struct Export;

impl Capability for Export {
    fn name(&self) -> &'static str {
        "registry.export"
    }
    fn effect(&self) -> Effect {
        Effect::ReversibleWrite
    }
    fn description(&self) -> &'static str {
        "Export everything the robot holds about the person -- all five \
         registry categories, item by item with sources -- as a JSON file \
         saved into their own files. Use when they ask to export, download \
         or take a copy of their data."
    }
    fn schema(&self) -> serde_json::Value {
        super::no_args()
    }
    fn validate(&self, _args: &serde_json::Value) -> Result<(), String> {
        Ok(())
    }
    fn execute(&self, ctx: &Ctx<'_>, _args: &serde_json::Value) -> Result<Outcome, PrismError> {
        let source = ctx.source_msg()?;
        let vault = ctx.vault()?;
        // full detail: every item, its source words, its class. This stays
        // INSIDE the contour -- a file in their own vault -- so the export
        // right costs no boundary crossing; getting it off the machine is
        // then their call, made with the file in hand.
        let body = ctx.cell.with(|c| {
            let mut full = overview_json(c).map_err(mind_err)?;
            let facts = mind::facts::registry_list(c, 1000).map_err(mind_err)?;
            full["knowledge"]["items"] = serde_json::json!(facts
                .iter()
                .map(|(f, source_words, _)| serde_json::json!({
                    "fact": f.content, "from_your_words": source_words,
                    "learned_at": f.created_at, "class": f.class,
                }))
                .collect::<Vec<_>>());
            let rules = mind::instructions::active(c).map_err(mind_err)?;
            full["instructions"]["items"] = serde_json::json!(rules
                .iter()
                .map(|i| serde_json::json!({ "rule": i.body, "since": i.created_at }))
                .collect::<Vec<_>>());
            let owed = mind::commitments::outstanding(c).map_err(mind_err)?;
            let settled = mind::commitments::recently_closed(c, 100).map_err(mind_err)?;
            full["knowledge"]["commitments"] = serde_json::json!({
                "outstanding": owed, "recently_closed": settled });
            full["media"]["items"] = serde_json::json!(mind::files::list(c)
                .map_err(mind_err)?
                .iter()
                .map(|f| serde_json::json!({ "name": f.name, "size": f.size, "class": f.class }))
                .collect::<Vec<_>>());
            serde_json::to_string_pretty(&full).map_err(|e| PrismError::Capability(e.to_string()))
        })?;
        let name = "registry export.json";
        let f = ctx.cell.with(|c| {
            let stored = vault
                .store(c, body.as_bytes(), Some("application/json"), Some(&source))
                .map_err(mind_err)?;
            mind::files::put(c, name, &stored.hash, stored.size, "owner_private", &source)
                .map_err(mind_err)
        })?;
        attested(
            super::row_evidence(&f.id, &f.hash),
            format!("exported the registry to {name} ({} bytes)", f.size),
            Rendering::new(
                "registry_exported",
                serde_json::json!({ "name": name, "size": f.size }),
            ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// THE GATE for gap item 8. A cell with every schema applied contains
    /// no table outside the declared map — so "nothing about you exists
    /// outside these five categories" is a property of the schema, and
    /// adding a store without answering "which category?" fails this test.
    #[test]
    fn every_cell_table_answers_to_a_category() {
        let conn = Connection::open_in_memory().unwrap();
        prism::init_cell_schema(&conn).unwrap();
        mind::init_cell_schema(&conn).unwrap();
        soul::init_cell_schema(&conn).unwrap();

        let unmapped = census(&conn).unwrap();
        assert!(
            unmapped.is_empty(),
            "tables outside the five categories -- which category is each, \
             and what rights apply? {unmapped:?}"
        );

        // and the map does not declare tables that stopped existing --
        // a stale map would vouch for things no longer there
        let mut stmt = conn
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table'")
            .unwrap();
        let real: Vec<String> = stmt
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        for (t, _) in CELL_TABLES {
            assert!(real.iter().any(|r| r == t), "map declares a ghost table: {t}");
        }
    }

    /// Each of the five §4b categories is actually populated by the map --
    /// a category with no tables would make the claim vacuous.
    #[test]
    fn all_five_categories_are_real() {
        for want in [
            Category::Knowledge,
            Category::Instructions,
            Category::Preferences,
            Category::Media,
            Category::Grants,
        ] {
            assert!(
                CELL_TABLES.iter().any(|(_, c)| *c == want),
                "{want:?} maps to no table"
            );
        }
    }
}
