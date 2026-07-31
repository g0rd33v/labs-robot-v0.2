//! Files, vault-scoped.
//!
//! Everything here lives in the person's own media vault (§4a): encrypted
//! under a key derived from their cell's DEK, content-addressed, and
//! travelling in the Robot Package like the rest of their state. There is
//! no path from these capabilities to the host filesystem — a robot that
//! can be asked to read `/etc/passwd` has a file-read primitive, not a file
//! capability.
//!
//! Names are checked, not trusted. `clean_name` is the whole boundary
//! between "a document called packing list.md" and "an argument that
//! reaches the disk", so it strips separators rather than rejecting them:
//! a refusal teaches the caller to try the next encoding, while a name that
//! simply cannot contain a separator has nothing to encode around.

use super::{attested, mind_err, note_evidence, typed, Capability, Ctx};
use prism::types::{Effect, Outcome, Rendering};
use prism::PrismError;
use serde::Deserialize;
use trust::classes::DataClass;

fn clean_name(raw: &str) -> Result<String, String> {
    let name: String = raw
        .trim()
        .chars()
        .filter(|c| !matches!(c, '/' | '\\' | '\0'))
        .collect();
    let name = name.trim_matches('.').trim().to_string();
    if name.is_empty() {
        return Err("a file needs a name".into());
    }
    if name.chars().count() > 120 {
        return Err("that name is too long".into());
    }
    Ok(name)
}

/// Files are documents, so the size that matters is "a person wrote this",
/// not "a model can emit this". Half a megabyte of text is a long book.
const MAX_BYTES: usize = 512 * 1024;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SaveArgs {
    name: String,
    content: String,
    #[serde(default)]
    class: Option<String>,
}

pub struct Save;

impl Capability for Save {
    fn name(&self) -> &'static str {
        "file.save"
    }
    fn effect(&self) -> Effect {
        Effect::ReversibleWrite
    }
    fn description(&self) -> &'static str {
        "Save text as a file in the person's own encrypted storage -- notes, \
         drafts, lists, snippets, anything they want kept as a document \
         rather than as a remembered fact. Use when they ask you to save, \
         write down, or put something in a file. Saving under a name that \
         already exists replaces its contents."
    }
    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "What to call it, for example 'packing list.md'. \
                                    A name, not a path."
                },
                "content": {
                    "type": "string",
                    "description": "The text to save. If the person dictated it, \
                                    keep their exact words."
                },
                "class": {
                    "type": "string",
                    "enum": ["public", "owner_private", "sensitive",
                             "restricted", "local_only"],
                    "description": "How sensitive the contents are. Omit unless \
                                    the person indicated. 'restricted' and \
                                    'local_only' never reach an external model."
                }
            },
            "required": ["name", "content"],
            "additionalProperties": false
        })
    }
    fn validate(&self, args: &serde_json::Value) -> Result<(), String> {
        let a: SaveArgs = typed(args)?;
        clean_name(&a.name)?;
        if a.content.is_empty() {
            return Err("nothing to save".into());
        }
        if a.content.len() > MAX_BYTES {
            return Err("that is too large to save as a file".into());
        }
        if let Some(c) = &a.class {
            DataClass::parse(c).ok_or_else(|| format!("no data class {c}"))?;
        }
        Ok(())
    }
    fn execute(&self, ctx: &Ctx<'_>, args: &serde_json::Value) -> Result<Outcome, PrismError> {
        let a: SaveArgs = typed(args).map_err(PrismError::Capability)?;
        let name = clean_name(&a.name).map_err(PrismError::Capability)?;
        let class = a
            .class
            .as_deref()
            .and_then(DataClass::parse)
            .unwrap_or_default();
        let source = ctx.source_msg()?;
        let vault = ctx.vault()?;

        let f = ctx.cell.with(|c| {
            let stored = vault
                .store(c, a.content.as_bytes(), Some("text/plain"), Some(&source))
                .map_err(mind_err)?;
            mind::files::put(
                c,
                &name,
                &stored.hash,
                stored.size,
                class.as_str(),
                &source,
            )
            .map_err(mind_err)
        })?;

        attested(
            super::row_evidence(&f.id, &f.hash),
            format!("saved {} bytes as {name}", f.size),
            Rendering::new(
                "file_saved",
                serde_json::json!({ "name": name, "size": f.size }),
            ),
        )
    }
}

pub struct List;

impl Capability for List {
    fn name(&self) -> &'static str {
        "file.list"
    }
    fn effect(&self) -> Effect {
        Effect::Read
    }
    fn description(&self) -> &'static str {
        "List the files the person has saved, most recently changed first. \
         Use when they ask what files they have or what you saved for them."
    }
    fn schema(&self) -> serde_json::Value {
        super::no_args()
    }
    fn validate(&self, _args: &serde_json::Value) -> Result<(), String> {
        Ok(())
    }
    fn execute(&self, ctx: &Ctx<'_>, _args: &serde_json::Value) -> Result<Outcome, PrismError> {
        let all = ctx
            .cell
            .with(|c| mind::files::list(c).map_err(mind_err))?;
        let say = if all.is_empty() {
            Rendering::bare("file_list_empty")
        } else {
            let items: Vec<serde_json::Value> = all
                .iter()
                .map(|f| serde_json::json!({ "name": f.name, "size": f.size }))
                .collect();
            Rendering::new("file_list", serde_json::json!({ "items": items }))
        };
        attested(
            note_evidence("file.list"),
            format!("listed {} files", all.len()),
            say,
        )
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct NameArgs {
    name: String,
}

fn name_schema(what: &str) -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "name": { "type": "string", "description": what }
        },
        "required": ["name"],
        "additionalProperties": false
    })
}

pub struct Read;

impl Capability for Read {
    fn name(&self) -> &'static str {
        "file.read"
    }
    fn effect(&self) -> Effect {
        Effect::Read
    }
    fn description(&self) -> &'static str {
        "Read back a file the person saved, by name. Use when they ask what \
         a file says, or when they want to work with something they saved."
    }
    fn schema(&self) -> serde_json::Value {
        name_schema("The file's name, as it was saved.")
    }
    fn validate(&self, args: &serde_json::Value) -> Result<(), String> {
        let a: NameArgs = typed(args)?;
        clean_name(&a.name).map(|_| ())
    }
    fn execute(&self, ctx: &Ctx<'_>, args: &serde_json::Value) -> Result<Outcome, PrismError> {
        let a: NameArgs = typed(args).map_err(PrismError::Capability)?;
        let want = clean_name(&a.name).map_err(PrismError::Capability)?;
        let vault = ctx.vault()?;

        let found = ctx.cell.with(|c| {
            match mind::files::get(c, &want).map_err(mind_err)? {
                Some(f) => {
                    let bytes = vault.get(c, &f.hash).map_err(mind_err)?;
                    Ok(bytes.map(|b| (f, b)))
                }
                None => Ok(None),
            }
        })?;

        match found {
            Some((f, bytes)) => {
                let text = String::from_utf8_lossy(&bytes).to_string();
                attested(
                    super::row_evidence(&f.id, &f.hash),
                    format!("read {} bytes from {want}", f.size),
                    Rendering::new(
                        "file_read",
                        serde_json::json!({ "name": want, "content": text }),
                    ),
                )
            }
            None => attested(
                note_evidence("file.read"),
                format!("no file named {want}"),
                Rendering::new("file_missing", serde_json::json!({ "name": want })),
            ),
        }
    }
}

pub struct Delete;

impl Capability for Delete {
    fn name(&self) -> &'static str {
        "file.delete"
    }
    fn effect(&self) -> Effect {
        Effect::ReversibleWrite
    }
    fn description(&self) -> &'static str {
        "Delete a file the person saved. Use when they ask you to delete, \
         remove or get rid of a file."
    }
    fn schema(&self) -> serde_json::Value {
        name_schema("The name of the file to delete.")
    }
    fn validate(&self, args: &serde_json::Value) -> Result<(), String> {
        let a: NameArgs = typed(args)?;
        clean_name(&a.name).map(|_| ())
    }
    fn execute(&self, ctx: &Ctx<'_>, args: &serde_json::Value) -> Result<Outcome, PrismError> {
        let a: NameArgs = typed(args).map_err(PrismError::Capability)?;
        let want = clean_name(&a.name).map_err(PrismError::Capability)?;
        let origin = ctx.origin().to_string();
        let gone = ctx
            .cell
            .with(|c| mind::files::delete(c, &want, &origin).map_err(mind_err))?;
        if gone {
            attested(
                note_evidence("file.delete"),
                format!("deleted {want}"),
                Rendering::new("file_deleted", serde_json::json!({ "name": want })),
            )
        } else {
            attested(
                note_evidence("file.delete"),
                format!("no file named {want}"),
                Rendering::new("file_missing", serde_json::json!({ "name": want })),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A name is a name. If it could carry a separator, `file.read` would be
    /// a host-filesystem read primitive wearing a capability's clothes.
    #[test]
    fn a_name_can_never_become_a_path() {
        for hostile in [
            "../../etc/passwd",
            "/etc/passwd",
            "..\\..\\windows\\system32",
            "notes/../../../secret",
            "....//....//etc/shadow",
        ] {
            let cleaned = clean_name(hostile).unwrap();
            assert!(!cleaned.contains('/'), "{hostile} -> {cleaned}");
            assert!(!cleaned.contains('\\'), "{hostile} -> {cleaned}");
            assert!(!cleaned.starts_with('.'), "{hostile} -> {cleaned}");
            assert!(!cleaned.contains('\0'), "{hostile} -> {cleaned}");
        }
        assert!(clean_name("   ").is_err());
        assert!(clean_name("///").is_err());
        assert!(clean_name("...").is_err());
        assert!(clean_name(&"x".repeat(200)).is_err());
        assert_eq!(clean_name(" packing list.md ").unwrap(), "packing list.md");
    }

    #[test]
    fn arguments_are_checked_before_anything_is_written() {
        assert!(Save
            .validate(&serde_json::json!({"name": "a.md", "content": "hi"}))
            .is_ok());
        assert!(Save
            .validate(&serde_json::json!({"name": "a.md", "content": ""}))
            .is_err());
        assert!(Save
            .validate(&serde_json::json!({"name": "/", "content": "hi"}))
            .is_err());
        assert!(Save
            .validate(&serde_json::json!({"name": "a", "content": "x", "class": "nonsense"}))
            .is_err());
        assert!(Save
            .validate(&serde_json::json!({"name": "a", "content": "x", "class": "restricted"}))
            .is_ok());
        assert!(
            Save.validate(&serde_json::json!({
                "name": "big", "content": "x".repeat(MAX_BYTES + 1)
            }))
            .is_err(),
            "a file is a document, not a dump"
        );
    }
}
