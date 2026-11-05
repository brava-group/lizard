//! Bake the UI string catalogs (`locales/<code>.toml`, see docs/translating.md) into sorted
//! static tables for `i18n`.

use std::fmt::Write as _;

fn main() {
    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap_or_default()).join("locales.rs");
    if let Err(e) = std::fs::write(&out, locales()) {
        println!("cargo::warning=writing {}: {e}", out.display());
    }
}

/// `pub static LANGS: &[Lang]`: every `locales/*.toml`, English first, each table sorted by key.
fn locales() -> String {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("locales");
    println!("cargo::rerun-if-changed={}", dir.display());
    let mut files: Vec<_> =
        std::fs::read_dir(&dir).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "toml")).collect();
    // By code, so "es" comes before "es-419".
    files.sort_by_key(|p| (p.file_stem().is_none_or(|s| s != "en"), p.file_stem().map(ToOwned::to_owned)));
    let langs: Vec<_> = files
        .into_iter()
        .filter_map(|path| {
            let t = read(&path, "the language is left out", |t| t.parse::<toml::Table>().map_err(|e| e.to_string()))?;
            Some((path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(), path, t))
        })
        .collect();
    let mut o = String::from(
        "pub static LANGS: &[Lang] = &[
",
    );
    for (code, path, t) in &langs {
        let meta = |k: &str| t.get("meta").and_then(|m| m.get(k)).and_then(toml::Value::as_str).unwrap_or("").to_string();
        // A regional catalog ("es-419") overlays its language's ("es"), if there is one.
        let parent = code.split_once('-').and_then(|(base, _)| langs.iter().position(|(c, ..)| c == base)).unwrap_or(0);
        let _ = writeln!(o, "    Lang {{ code: {code:?}, name: {:?}, draft: {}, parent: {parent}, ui: &[", meta("name"), !meta("status").is_empty());
        table(&mut o, path, t.get("ui"));
        let _ = writeln!(o, "    ], commands: &[");
        table(&mut o, path, t.get("commands"));
        let _ = writeln!(o, "    ], menu: &[");
        table(&mut o, path, t.get("menu"));
        let _ = writeln!(o, "    ] }},");
    }
    o.push_str(
        "];
",
    );
    o
}

/// The rows of one catalog table: strings or plural forms (`other` required).
fn table(o: &mut String, path: &std::path::Path, t: Option<&toml::Value>) {
    let Some(t) = t.and_then(toml::Value::as_table) else { return };
    // Sorted by key (byte order), as the lookup's binary search needs.
    let mut rows: Vec<_> = t.iter().collect();
    rows.sort_by(|a, b| a.0.cmp(b.0));
    for (k, v) in rows {
        let s = |k: &str| v.get(k).and_then(toml::Value::as_str);
        match (v.as_str(), s("one"), s("other")) {
            (Some(text), ..) => _ = writeln!(o, "        ({k:?}, Text::Plain({text:?})),"),
            (None, one, Some(other)) => {
                _ = writeln!(
                    o,
                    "        ({k:?}, Text::Plural {{ one: {}, few: {}, many: {}, other: {other:?} }}),",
                    name(one),
                    name(s("few")),
                    name(s("many")),
                );
            }
            _ => println!("cargo::warning={}: `{k}` is neither a string nor plural forms with `other`; left out", path.display()),
        }
    }
}

fn read<T>(path: &std::path::Path, fallback: &str, parse: impl Fn(&str) -> Result<T, String>) -> Option<T> {
    println!("cargo::rerun-if-changed={}", path.display());
    let r = std::fs::read_to_string(path).map_err(|e| e.to_string()).and_then(|t| parse(&t));
    if let Err(e) = &r {
        println!("cargo::warning={}: {e}; {fallback}", path.display());
    }
    r.ok()
}

fn name(v: Option<&str>) -> String {
    match v.map(str::trim).filter(|s| !s.is_empty()) {
        Some(x) => format!("Some({x:?})"),
        None => "None".into(),
    }
}
