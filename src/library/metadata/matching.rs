//! Exact disc identification and user-confirmed matches from the local release index.
use super::{Library, Release, evidence, wikidata};
use rusqlite::{Connection, Transaction, params};
use std::collections::HashSet;

pub(super) fn matching_serial<'a>(
    rows: &'a [Release],
    serial: Option<&str>,
    region: Option<&str>,
) -> Option<&'a Release> {
    let serial = serial?;
    let mut exact = rows
        .iter()
        .filter(|r| r.serial == serial && region.is_none_or(|v| v == r.region));
    let found = exact.next()?;
    exact.next().is_none().then_some(found)
}

fn normalized_title(title: &str) -> String {
    let mut name = String::with_capacity(title.len());
    for token in title
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
    {
        if !name.is_empty() {
            name.push(' ');
        }
        name.extend(token.chars().flat_map(char::to_lowercase));
    }
    name
}

pub(super) fn title_hint(title: &str) -> String {
    let mut title = title.trim();
    while let Some((open, close)) = title.chars().last().and_then(|c| match c {
        ')' => Some(('(', ')')),
        ']' => Some(('[', ']')),
        _ => None,
    }) {
        let Some((base, tag)) = title.trim_end_matches(close).rsplit_once(open) else {
            break;
        };
        let tag = normalized_title(tag);
        if open == '('
            && !tag.split_whitespace().all(|word| {
                matches!(
                    word,
                    "usa"
                        | "europe"
                        | "japan"
                        | "uk"
                        | "us"
                        | "eu"
                        | "jp"
                        | "en"
                        | "ja"
                        | "fr"
                        | "de"
                        | "es"
                        | "it"
                        | "disc"
                        | "disk"
                        | "rev"
                        | "version"
                ) || word.chars().all(|c| c.is_ascii_digit())
            })
        {
            break;
        }
        title = base.trim();
    }
    normalized_title(title)
}

pub(super) fn has_title_candidate(conn: &Connection, hint: &str) -> rusqlite::Result<bool> {
    if hint.is_empty() {
        return Ok(false);
    }
    conn.prepare_cached("SELECT 1 FROM release_names WHERE name=?")?
        .exists([hint])
}

pub(super) fn index_names(tx: &Transaction<'_>, release: &Release) -> rusqlite::Result<()> {
    let mut insert =
        tx.prepare_cached("INSERT OR IGNORE INTO release_names(name,release_id) VALUES(?,?)")?;
    for name in release.names() {
        let key = normalized_title(name);
        if !key.is_empty() {
            insert.execute(params![key, release.id])?;
        }
    }
    Ok(())
}

pub fn candidates(lib: &Library, id: i64, query: &str) -> Result<Vec<Release>, String> {
    let conn = lib.connect()?;
    let evidence = evidence(&conn, id).map_err(|e| e.to_string())?;
    let needle = query.trim();
    let mut result = Vec::new();
    let mut stmt = if needle.is_empty() {
        conn.prepare_cached(
            "SELECT id,serial,region,title,date,aliases,work_date FROM fetched_releases
             WHERE serial=?1 OR id IN (SELECT release_id FROM release_names WHERE name=?2) ORDER BY id")
    } else {
        conn.prepare_cached(
            "SELECT id,serial,region,title,date,aliases,work_date FROM fetched_releases
             WHERE instr(serial,?1)>0 OR id IN (SELECT release_id FROM release_names WHERE ?2<>'' AND instr(name,?2)>0)
             ORDER BY (serial=?1) DESC, title, id LIMIT 100")
    }.map_err(|e| e.to_string())?;
    let (serial, name) = if needle.is_empty() {
        (
            evidence.serial.unwrap_or_default(),
            title_hint(&evidence.filename),
        )
    } else {
        (needle.to_ascii_uppercase(), normalized_title(needle))
    };
    for row in stmt
        .query_map(params![serial, name], Release::from_row)
        .map_err(|e| e.to_string())?
    {
        result.push(row.map_err(|e| e.to_string())?);
    }
    if result.is_empty() && !needle.is_empty() && !wikidata::is_supported_serial(&serial) {
        return fuzzy_suggestions(&conn, &name).map_err(|e| e.to_string());
    }
    Ok(result)
}

fn trigrams(text: &str) -> HashSet<[char; 3]> {
    let mut chars = text.chars();
    let mut prev = [
        chars.next().unwrap_or_default(),
        chars.next().unwrap_or_default(),
    ];
    chars
        .map(|ch| {
            let gram = [prev[0], prev[1], ch];
            prev = [prev[1], ch];
            gram
        })
        .collect()
}
/// Shortlisted, user-confirmed suggestions only. Character trigrams work with
/// Japanese titles as well as Latin words and avoid sending local filenames upstream.
fn fuzzy_suggestions(conn: &Connection, needle: &str) -> rusqlite::Result<Vec<Release>> {
    const MAX_SUGGESTIONS: usize = 8;
    let length = needle.chars().count();
    if !(5..=128).contains(&length) {
        return Ok(Vec::new());
    }
    let wanted = trigrams(needle);
    let mut best: Vec<(usize, String)> = Vec::new();
    let mut keys = conn.prepare_cached(
        "SELECT name,release_id FROM release_names WHERE length(name) BETWEEN ?1 AND ?2",
    )?;
    let mut rows = keys.query(params![length * 3 / 4, length * 5 / 4 + 1])?;
    while let Some(row) = rows.next()? {
        let key = trigrams(row.get_ref(0)?.as_str()?);
        // Sørensen–Dice similarity of at least 75% is required even for a suggestion.
        let similarity = 200 * wanted.intersection(&key).count() / (wanted.len() + key.len());
        if similarity < 75 {
            continue;
        }
        let id = row.get_ref(1)?.as_str()?;
        if best
            .iter()
            .any(|(score, existing)| existing == id && *score >= similarity)
        {
            continue;
        }
        best.retain(|(_, existing)| existing != id);
        best.push((similarity, id.to_owned()));
        best.sort_unstable_by(|(a, aid), (b, bid)| b.cmp(a).then_with(|| aid.cmp(bid)));
        best.truncate(MAX_SUGGESTIONS);
    }
    let mut details = conn.prepare_cached(
        "SELECT id,serial,region,title,date,aliases,work_date FROM fetched_releases WHERE id=?",
    )?;
    best.into_iter()
        .map(|(_, id)| details.query_row([id], Release::from_row))
        .collect()
}
