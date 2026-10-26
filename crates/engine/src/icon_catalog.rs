//! Offline, redistributable UI icon catalog used by both the command engine and MCP search.
//!
//! The source packages and their licences are pinned in `assets/icon-packs/`. We store the
//! library/name on each placed icon's root layer and keep editable vector geometry in the file,
//! so a document remains renderable even if this catalog changes in a future release.

use std::collections::BTreeMap;
use std::io::Read;
use std::sync::OnceLock;

use flate2::read::GzDecoder;
use serde::{Deserialize, Serialize};

const DATA: &[u8] = include_bytes!("../../../assets/icon-packs/catalog.json.gz");
const MAX_UNCOMPRESSED: u64 = 64 * 1024 * 1024;

pub const FAMILIES: [&str; 6] = ["lucide", "feather", "phosphor", "material-symbols-outlined", "material-symbols-rounded", "material-symbols-sharp"];

#[derive(Deserialize)]
struct Catalog {
    schema: u32,
    icons: BTreeMap<String, BTreeMap<String, String>>,
}

fn load() -> Result<Catalog, String> {
    let mut bytes = Vec::new();
    GzDecoder::new(DATA).take(MAX_UNCOMPRESSED + 1).read_to_end(&mut bytes).map_err(|e| format!("icon catalog decompression failed: {e}"))?;
    if bytes.len() as u64 > MAX_UNCOMPRESSED {
        return Err("icon catalog is too large".into());
    }
    let catalog: Catalog = serde_json::from_slice(&bytes).map_err(|e| format!("icon catalog is invalid: {e}"))?;
    if catalog.schema != 1 || FAMILIES.iter().any(|family| !catalog.icons.contains_key(*family)) {
        return Err("icon catalog has an unsupported schema or missing family".into());
    }
    Ok(catalog)
}

fn catalog() -> Result<&'static Catalog, String> {
    static CATALOG: OnceLock<Result<Catalog, String>> = OnceLock::new();
    CATALOG.get_or_init(load).as_ref().map_err(Clone::clone)
}

fn normalized(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut separator = false;
    for c in value.trim().chars() {
        if c.is_ascii_alphanumeric() {
            if separator && !out.is_empty() {
                out.push('-');
            }
            separator = false;
            out.push(c.to_ascii_lowercase());
        } else {
            separator = true;
        }
    }
    out
}

/// Accept the labels used by pen.dev as well as the stable Lizard IDs.
pub fn family_id(value: &str) -> Option<&'static str> {
    match normalized(value).as_str() {
        "lucide" | "lucide-icons" => Some("lucide"),
        "feather" | "feather-icons" => Some("feather"),
        "phosphor" | "phosphor-icons" => Some("phosphor"),
        "material-symbols-outlined" => Some("material-symbols-outlined"),
        "material-symbols-rounded" => Some("material-symbols-rounded"),
        "material-symbols-sharp" => Some("material-symbols-sharp"),
        _ => None,
    }
}

/// SVG data for an exact icon name. Icon names use kebab-case; underscores and spaces are also
/// accepted for Material Symbols names.
pub fn svg(family: &str, name: &str) -> Result<Option<&'static str>, String> {
    let Some(family) = family_id(family) else { return Ok(None) };
    Ok(catalog()?.icons.get(family).and_then(|icons| icons.get(&normalized(name))).map(String::as_str))
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IconHit {
    pub library: &'static str,
    pub name: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResult {
    pub query: String,
    pub preferred_library: Option<&'static str>,
    /// True only when no useful match exists in the preferred family.
    pub used_fallback: bool,
    pub matches: Vec<IconHit>,
}

fn alternate_names(query: &str) -> &'static [&'static str] {
    match query {
        "home" | "inicio" | "in-icio" => &["house"],
        "settings" | "configuracoes" => &["gear", "cog"],
        "user" | "usuario" => &["person"],
        "users" | "usuarios" => &["people", "user-group"],
        "delete" | "remove" | "excluir" => &["trash", "trash-2", "trash-can"],
        "add" | "adicionar" => &["plus"],
        "close" | "fechar" => &["x"],
        "search" | "buscar" => &["magnifying-glass"],
        "edit" | "editar" => &["pencil"],
        "check" | "confirmar" => &["checkmark"],
        _ => &[],
    }
}

fn rank(name: &str, query: &str) -> Option<u8> {
    if name == query {
        return Some(0);
    }
    if alternate_names(query).contains(&name) {
        return Some(1);
    }
    if name.starts_with(query) || name.split('-').any(|part| part == query) {
        return Some(2);
    }
    if name.contains(query) {
        return Some(3);
    }
    let words: Vec<&str> = query.split('-').filter(|part| !part.is_empty()).collect();
    (words.len() > 1 && words.iter().all(|part| name.contains(part))).then_some(4)
}

/// Return matches from `preferred` whenever it contains the requested icon or a name match.
/// Other families are searched only when the preferred family has no match.
pub fn search(query: &str, preferred: Option<&str>, limit: usize) -> Result<SearchResult, String> {
    let query = normalized(query);
    let preferred_library = match preferred {
        Some(value) => Some(family_id(value).ok_or_else(|| format!("unknown icon library `{value}`"))?),
        None => None,
    };
    let data = catalog()?;
    let gather = |families: &[&'static str]| {
        let mut found = Vec::new();
        if !query.is_empty() {
            for (family_order, family) in families.iter().enumerate() {
                if let Some(icons) = data.icons.get(*family) {
                    for name in icons.keys() {
                        if let Some(score) = rank(name, &query) {
                            found.push((score, family_order, IconHit { library: family, name: name.clone() }));
                        }
                    }
                }
            }
        }
        found.sort_by(|a, b| (a.0, a.1, &a.2.name).cmp(&(b.0, b.1, &b.2.name)));
        found.into_iter().take(limit.clamp(1, 40)).map(|(_, _, hit)| hit).collect::<Vec<_>>()
    };
    let first = preferred_library.map_or_else(Vec::new, |family| gather(&[family]));
    let used_fallback = preferred_library.is_some() && first.is_empty();
    let matches = if preferred_library.is_some() && !used_fallback { first } else { gather(&FAMILIES) };
    Ok(SearchResult { query, preferred_library, used_fallback, matches })
}

pub fn counts() -> Result<Vec<(&'static str, usize)>, String> {
    let data = catalog()?;
    Ok(FAMILIES.iter().map(|family| (*family, data.icons.get(*family).map_or(0, BTreeMap::len))).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_family_has_a_real_svg() -> Result<(), String> {
        for family in FAMILIES {
            let name = if family == "phosphor" { "house" } else { "home" };
            let icon = svg(family, name)?.ok_or_else(|| format!("missing {family}/{name}"))?;
            assert!(icon.contains("<svg"));
        }
        Ok(())
    }

    #[test]
    fn preference_holds_until_an_icon_is_missing() -> Result<(), String> {
        let same = search("home", Some("lucide"), 8)?;
        assert!(!same.used_fallback);
        assert!(same.matches.iter().all(|hit| hit.library == "lucide"));
        let missing = search("analytics", Some("feather"), 8)?;
        assert!(missing.used_fallback);
        assert!(missing.matches.iter().any(|hit| hit.name == "analytics"));
        Ok(())
    }
}
