use std::path::Path;

use sha2::{Digest, Sha256};

use crate::Error;
use crate::embed::Embedder;
use crate::store::{Row, Store};

const CHUNK_WORDS: usize = 800;
const CHUNK_OVERLAP: usize = 100;

pub(crate) fn chunk_text(text: &str) -> Vec<String> {
    let paragraphs: Vec<String> = text
        .split("\n\n")
        .map(str::trim)
        .filter(|paragraph| !paragraph.is_empty())
        .map(str::to_owned)
        .collect();
    if paragraphs.is_empty() {
        return slide(&words(text));
    }
    let mut chunks = Vec::new();
    let mut current: Vec<String> = Vec::new();
    let mut count = 0usize;
    for paragraph in paragraphs {
        let paragraph_words = words(&paragraph);
        if paragraph_words.len() > CHUNK_WORDS {
            if !current.is_empty() {
                chunks.push(current.join("\n\n"));
                current.clear();
                count = 0;
            }
            chunks.extend(slide(&paragraph_words));
            continue;
        }
        if count + paragraph_words.len() > CHUNK_WORDS && count > 0 {
            chunks.push(current.join("\n\n"));
            current = tail_overlap(&current);
            count = current.iter().map(|item| words(item).len()).sum();
        }
        count += paragraph_words.len();
        current.push(paragraph);
    }
    if !current.is_empty() {
        chunks.push(current.join("\n\n"));
    }
    chunks
}

fn tail_overlap(paragraphs: &[String]) -> Vec<String> {
    let Some(last) = paragraphs.last() else {
        return Vec::new();
    };
    let last_words = words(last);
    if last_words.len() <= CHUNK_OVERLAP {
        return vec![last.clone()];
    }
    vec![last_words[last_words.len() - CHUNK_OVERLAP..].join(" ")]
}

fn slide(words: &[&str]) -> Vec<String> {
    if words.is_empty() {
        return Vec::new();
    }
    if words.len() <= CHUNK_WORDS {
        return vec![words.join(" ")];
    }
    let mut chunks = Vec::new();
    let mut start = 0usize;
    while start < words.len() {
        let end = (start + CHUNK_WORDS).min(words.len());
        chunks.push(words[start..end].join(" "));
        if end == words.len() {
            break;
        }
        start = end - CHUNK_OVERLAP;
    }
    chunks
}

fn words(text: &str) -> Vec<&str> {
    text.split_whitespace().collect()
}

pub(crate) fn content_hash(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

pub(crate) struct Ingested {
    pub doc_id: String,
}

pub(crate) async fn ingest_text(
    store: &mut Store,
    embedder: &mut dyn Embedder,
    text: &str,
    title: &str,
    author: &str,
    path: Option<&str>,
    now: i64,
    source_time: i64,
) -> Result<Ingested, Error> {
    if text.trim().is_empty() {
        return Err(Error::EmptyText);
    }
    let meta = store
        .read_meta()
        .await?
        .ok_or_else(|| Error::Store("directory has no model lock".into()))?;
    let hash = content_hash(text);
    let docs = store.docs().await?;
    if let Some(existing) = docs
        .iter()
        .find(|row| row.content_hash.as_deref() == Some(hash.as_str()))
    {
        return Ok(Ingested {
            doc_id: existing.id.clone(),
        });
    }
    let superseded = superseded_ids(&docs);
    let previous = path.and_then(|path| {
        docs.iter()
            .filter(|row| row.path.as_deref() == Some(path) && !superseded.contains(&row.id))
            .max_by_key(|row| row.ingested_at)
            .map(|row| row.id.clone())
    });
    let doc_id = ulid::Ulid::new().to_string();
    let chunks = chunk_text(text);
    if chunks.is_empty() {
        return Err(Error::EmptyText);
    }
    let vectors = tokio::task::block_in_place(|| embedder.embed(&chunks))?;
    let dim = i32::try_from(meta.dim).map_err(|_| Error::Store("dim does not fit".into()))?;
    let mut rows = Vec::with_capacity(chunks.len() + 1);
    rows.push(Row {
        id: doc_id.clone(),
        kind: "doc".into(),
        doc_id: None,
        path: path.map(str::to_owned),
        title: title.to_owned(),
        text: text.to_owned(),
        author: author.to_owned(),
        created_at: source_time,
        ingested_at: now,
        supersedes: previous,
        source_doc_id: None,
        links: None,
        model: meta.model.clone(),
        dim,
        content_hash: Some(hash),
        vector: None,
    });
    for (chunk, vector) in chunks.into_iter().zip(vectors) {
        rows.push(Row {
            id: ulid::Ulid::new().to_string(),
            kind: "chunk".into(),
            doc_id: Some(doc_id.clone()),
            path: None,
            title: title.to_owned(),
            text: chunk,
            author: author.to_owned(),
            created_at: source_time,
            ingested_at: now,
            supersedes: None,
            source_doc_id: None,
            links: None,
            model: meta.model.clone(),
            dim,
            content_hash: None,
            vector: Some(vector),
        });
    }
    store.insert(&rows).await?;
    Ok(Ingested { doc_id })
}

pub(crate) fn superseded_ids(docs: &[Row]) -> std::collections::HashSet<String> {
    docs.iter()
        .filter_map(|row| row.supersedes.clone())
        .collect()
}

pub(crate) fn read_file(path: &Path) -> Result<(String, i64), Error> {
    let bytes = std::fs::read(path)?;
    let text = String::from_utf8(bytes)
        .map_err(|_| Error::Store(format!("{} is not utf-8", path.display())))?;
    let modified = std::fs::metadata(path)?.modified()?;
    let created = i64::try_from(
        modified
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|err| Error::Store(err.to_string()))?
            .as_micros(),
    )
    .unwrap_or(i64::MAX);
    Ok((text, created))
}
