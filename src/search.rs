use std::collections::HashMap;

use schemars::JsonSchema;
use serde::Serialize;

use crate::Error;
use crate::embed::Embedder;
use crate::ingest::superseded_ids;
use crate::store::{ChunkHit, Store};

const FUSION_K: f32 = 60.0;
const EXCERPT_CHARS: usize = 400;

#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
pub struct Hit {
    pub doc_id: String,
    pub title: String,
    pub path: String,
    pub score: f32,
    pub excerpt: String,
}

pub(crate) async fn search(
    store: &mut Store,
    embedder: &mut dyn Embedder,
    query: &str,
    k: usize,
    include_superseded: bool,
) -> Result<Vec<Hit>, Error> {
    if query.trim().is_empty() {
        return Err(Error::EmptyText);
    }
    let vectors = tokio::task::block_in_place(|| embedder.embed(&[query.to_owned()]))?;
    let vector = vectors
        .into_iter()
        .next()
        .ok_or_else(|| Error::Embed("embedder returned no query vector".into()))?;
    let by_vector = store.vector_search(&vector).await?;
    let by_text = store.text_search(query).await?;
    let fused = fuse(&by_vector, &by_text);
    let docs = store.docs().await?;
    let hidden = superseded_ids(&docs);
    let mut best: HashMap<String, (f32, String)> = HashMap::new();
    for (hit, score) in fused {
        if !include_superseded && hidden.contains(&hit.doc_id) {
            continue;
        }
        let entry = best
            .entry(hit.doc_id.clone())
            .or_insert((score, hit.text.clone()));
        if score > entry.0 {
            *entry = (score, hit.text);
        }
    }
    let mut hits = Vec::new();
    for (doc_id, (score, excerpt)) in best {
        let Some(doc) = docs.iter().find(|row| row.id == doc_id) else {
            continue;
        };
        hits.push(Hit {
            doc_id,
            title: doc.title.clone(),
            path: doc.path.clone().unwrap_or_default(),
            score,
            excerpt: excerpt_of(&excerpt),
        });
    }
    hits.sort_by(|left, right| {
        right
            .score
            .total_cmp(&left.score)
            .then_with(|| left.doc_id.cmp(&right.doc_id))
    });
    hits.truncate(k);
    Ok(hits)
}

fn fuse(by_vector: &[ChunkHit], by_text: &[ChunkHit]) -> Vec<(ChunkHit, f32)> {
    let mut scores: HashMap<String, (ChunkHit, f32)> = HashMap::new();
    add_ranks(&mut scores, by_vector);
    add_ranks(&mut scores, by_text);
    scores.into_values().collect()
}

fn add_ranks(scores: &mut HashMap<String, (ChunkHit, f32)>, hits: &[ChunkHit]) {
    for (offset, hit) in hits.iter().enumerate() {
        let rank = offset as f32 + 1.0;
        let score = 1.0 / (FUSION_K + rank);
        scores
            .entry(hit.id.clone())
            .and_modify(|entry| entry.1 += score)
            .or_insert_with(|| (hit.clone(), score));
    }
}

fn excerpt_of(text: &str) -> String {
    text.chars().take(EXCERPT_CHARS).collect()
}

#[cfg(test)]
mod tests {
    use super::fuse;
    use crate::store::ChunkHit;

    #[test]
    fn fusion_adds_both_lists() {
        let pasta = ChunkHit {
            id: "c1".into(),
            doc_id: "d1".into(),
            text: "pasta".into(),
        };
        let tax = ChunkHit {
            id: "c2".into(),
            doc_id: "d2".into(),
            text: "tax".into(),
        };
        let fused = fuse(&[pasta.clone(), tax.clone()], &[pasta]);
        let pasta_score = fused
            .iter()
            .find(|(hit, _)| hit.id == "c1")
            .map(|(_, score)| *score)
            .unwrap();
        let tax_score = fused
            .iter()
            .find(|(hit, _)| hit.id == "c2")
            .map(|(_, score)| *score)
            .unwrap();
        assert!(
            pasta_score > tax_score,
            "a chunk in both lists outranks a chunk in one, {pasta_score} vs {tax_score}"
        );
    }
}
