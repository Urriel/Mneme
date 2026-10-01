use std::collections::HashSet;
use std::fs::{File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use embed::{Embedder, FastEmbedder};

mod embed;
mod store;

use store::Store;

static OPEN_DIRS: LazyLock<Mutex<HashSet<PathBuf>>> = LazyLock::new(|| Mutex::new(HashSet::new()));

#[derive(Debug)]
pub enum BrainError {
    UnsupportedSchema(u32),
    ModelMismatch { expected: String, found: String },
    WriterBusy,
    EmptyText,
    EmptyQuery,
    BadLimit,
    MetadataNotObject,
    MetadataTooLarge,
    DimensionMismatch { expected: usize, found: usize },
    BadDistance,
    Embed(String),
    Store(String),
    Io(std::io::Error),
    CorruptLayout(String),
}

impl std::fmt::Display for BrainError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedSchema(version) => {
                write!(formatter, "schema version {version} is not supported")
            }
            Self::ModelMismatch { expected, found } => {
                write!(
                    formatter,
                    "model is {found}, this process embeds with {expected}"
                )
            }
            Self::WriterBusy => write!(formatter, "this data directory is already open"),
            Self::EmptyText => write!(formatter, "text is empty"),
            Self::EmptyQuery => write!(formatter, "query is empty"),
            Self::BadLimit => write!(formatter, "limit must be from 1 to 100"),
            Self::MetadataNotObject => write!(formatter, "metadata must be a JSON object"),
            Self::MetadataTooLarge => write!(formatter, "metadata is larger than 16 KiB"),
            Self::DimensionMismatch { expected, found } => {
                write!(formatter, "embedding width is {found}, expected {expected}")
            }
            Self::BadDistance => write!(formatter, "distance is negative or not finite"),
            Self::Embed(message) => write!(formatter, "embed failed. {message}"),
            Self::Store(message) => write!(formatter, "store failed. {message}"),
            Self::Io(err) => write!(formatter, "io failed. {err}"),
            Self::CorruptLayout(message) => {
                write!(formatter, "brain directory is corrupt. {message}")
            }
        }
    }
}

impl std::error::Error for BrainError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            _ => None,
        }
    }
}

impl From<std::io::Error> for BrainError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err)
    }
}

impl From<embed::EmbedError> for BrainError {
    fn from(err: embed::EmbedError) -> Self {
        Self::Embed(err.into_message())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Text(String);

impl Text {
    pub fn new(value: impl Into<String>) -> Result<Self, BrainError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(BrainError::EmptyText);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Query(String);

impl Query {
    pub fn new(value: impl Into<String>) -> Result<Self, BrainError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(BrainError::EmptyQuery);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Metadata(serde_json::Value);

impl Metadata {
    pub fn from_json(value: Option<serde_json::Value>) -> Result<Self, BrainError> {
        let value = value.unwrap_or_else(|| serde_json::json!({}));
        if !value.is_object() {
            return Err(BrainError::MetadataNotObject);
        }
        let compact = serde_json::to_string(&value)
            .map_err(|err| BrainError::Store(format!("metadata did not serialize: {err}")))?;
        if compact.len() > 16 * 1024 {
            return Err(BrainError::MetadataTooLarge);
        }
        Ok(Self(value))
    }

    pub(crate) fn as_value(&self) -> &serde_json::Value {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limit(usize);

impl Limit {
    pub fn new(value: Option<u32>) -> Result<Self, BrainError> {
        match value {
            None => Ok(Self(5)),
            Some(value) if (1..=100).contains(&value) => Ok(Self(value as usize)),
            Some(_) => Err(BrainError::BadLimit),
        }
    }

    pub(crate) fn get(self) -> usize {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemoryId(Uuid);

impl MemoryId {
    fn fresh() -> Self {
        Self(Uuid::new_v4())
    }

    pub(crate) fn parse(value: &str) -> Result<Self, BrainError> {
        Uuid::parse_str(value)
            .map(Self)
            .map_err(|err| BrainError::Store(format!("id is not a uuid: {err}")))
    }
}

impl std::fmt::Display for MemoryId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    pub id: MemoryId,
    pub text: String,
    pub score: f32,
    pub metadata: serde_json::Value,
}

struct OpenGuard {
    path: PathBuf,
    _file: File,
}

impl Drop for OpenGuard {
    fn drop(&mut self) {
        unclaim(&self.path);
    }
}

struct State {
    embedder: Box<dyn Embedder>,
    store: Store,
}

struct Shared {
    state: tokio::sync::Mutex<State>,
    _guard: OpenGuard,
}

#[derive(Clone)]
pub struct Brain {
    shared: Arc<Shared>,
}

impl Brain {
    pub async fn open(path: impl AsRef<Path>) -> Result<Self, BrainError> {
        let dimensions = FastEmbedder::default_dimensions()?;
        Self::open_in(path, FastEmbedder::ID, dimensions, || {
            let embedder = tokio::task::block_in_place(FastEmbedder::load)?;
            Ok(Box::new(embedder))
        })
        .await
    }

    #[cfg(test)]
    pub(crate) async fn open_with(
        path: impl AsRef<Path>,
        embedder: Box<dyn Embedder>,
    ) -> Result<Self, BrainError> {
        let model_id = embedder.id().to_owned();
        let dimensions = embedder.dimensions();
        Self::open_in(path, &model_id, dimensions, || Ok(embedder)).await
    }

    async fn open_in<F>(
        path: impl AsRef<Path>,
        model_id: &str,
        dimensions: usize,
        embedder: F,
    ) -> Result<Self, BrainError>
    where
        F: FnOnce() -> Result<Box<dyn Embedder>, BrainError>,
    {
        let guard = lock_dir(path.as_ref())?;
        prepare_layout(&guard.path, model_id)?;
        let store = Store::open(&guard.path, dimensions).await?;
        let embedder = embedder()?;
        Ok(Self {
            shared: Arc::new(Shared {
                state: tokio::sync::Mutex::new(State { embedder, store }),
                _guard: guard,
            }),
        })
    }

    pub async fn add(&self, text: Text, metadata: Metadata) -> Result<MemoryId, BrainError> {
        let mut state = self.shared.state.lock().await;
        let vector = tokio::task::block_in_place(|| state.embedder.embed(text.as_str()))?;
        let id = MemoryId::fresh();
        state
            .store
            .insert(id, text.as_str(), &vector, metadata.as_value())
            .await?;
        Ok(id)
    }

    pub async fn search(&self, query: Query, limit: Limit) -> Result<Vec<Hit>, BrainError> {
        let mut state = self.shared.state.lock().await;
        let vector = tokio::task::block_in_place(|| state.embedder.embed(query.as_str()))?;
        let rows = state.store.search(&vector, limit.get()).await?;
        rows.into_iter()
            .map(|row| {
                Ok(Hit {
                    id: row.id,
                    text: row.text,
                    score: score_from_l2(row.distance)?,
                    metadata: row.metadata,
                })
            })
            .collect()
    }
}

fn score_from_l2(distance: f32) -> Result<f32, BrainError> {
    if !distance.is_finite() || distance < 0.0 {
        return Err(BrainError::BadDistance);
    }
    Ok(1.0 / (1.0 + distance))
}

fn lock_dir(dir: &Path) -> Result<OpenGuard, BrainError> {
    std::fs::create_dir_all(dir)?;
    let path = dir.canonicalize()?;
    claim(&path)?;
    let file = match OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path.join("writer.lock"))
    {
        Ok(file) => file,
        Err(err) => {
            unclaim(&path);
            return Err(err.into());
        }
    };
    match file.try_lock() {
        Ok(()) => Ok(OpenGuard { path, _file: file }),
        Err(TryLockError::WouldBlock) => {
            unclaim(&path);
            Err(BrainError::WriterBusy)
        }
        Err(TryLockError::Error(err)) => {
            unclaim(&path);
            Err(err.into())
        }
    }
}

fn claim(path: &Path) -> Result<(), BrainError> {
    let mut open = OPEN_DIRS.lock().unwrap_or_else(|err| err.into_inner());
    if open.insert(path.to_path_buf()) {
        Ok(())
    } else {
        Err(BrainError::WriterBusy)
    }
}

fn unclaim(path: &Path) {
    let mut open = OPEN_DIRS.lock().unwrap_or_else(|err| err.into_inner());
    open.remove(path);
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BrainFile {
    schema_version: u32,
    model: String,
}

fn prepare_layout(dir: &Path, model_id: &str) -> Result<(), BrainError> {
    let toml_path = dir.join("brain.toml");
    let lance = dir.join("lance");
    if lance.exists() && !lance.is_dir() {
        return Err(BrainError::CorruptLayout("lance is not a directory".into()));
    }
    if !toml_path.is_file() {
        if lance.exists() {
            return Err(BrainError::CorruptLayout(
                "lance exists without brain.toml".into(),
            ));
        }
        write_brain_file(dir, model_id)?;
        return Ok(());
    }
    let raw = std::fs::read_to_string(&toml_path)?;
    let file: BrainFile =
        toml::from_str(&raw).map_err(|err| BrainError::CorruptLayout(err.to_string()))?;
    if file.schema_version != 1 {
        return Err(BrainError::UnsupportedSchema(file.schema_version));
    }
    if file.model != model_id {
        return Err(BrainError::ModelMismatch {
            expected: model_id.to_string(),
            found: file.model,
        });
    }
    Ok(())
}

fn write_brain_file(dir: &Path, model_id: &str) -> Result<(), BrainError> {
    let body = toml::to_string(&BrainFile {
        schema_version: 1,
        model: model_id.to_owned(),
    })
    .map_err(|err| BrainError::CorruptLayout(err.to_string()))?;
    let temporary = dir.join(".brain.toml.tmp");
    std::fs::write(&temporary, body)?;
    std::fs::rename(&temporary, dir.join("brain.toml"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brain::embed::fake;

    const PASTA: &str = "Boil salted water, add dried pasta, and simmer until tender.";
    const TAX: &str = "File the tax forms before the April deadline.";
    const QUERY: &str = "how do I cook noodles";

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("mneme-{}", Uuid::new_v4()));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn ranked_pairs() -> Vec<(&'static str, [f32; 4])> {
        vec![
            (PASTA, [1.0, 0.0, 0.0, 0.0]),
            (TAX, [0.0, 1.0, 0.0, 0.0]),
            (QUERY, [1.0, 0.0, 0.0, 0.0]),
        ]
    }

    #[test]
    fn score_maps_l2_distance() {
        assert_eq!(score_from_l2(0.0).unwrap(), 1.0, "distance 0 scores 1");
        assert_eq!(score_from_l2(1.0).unwrap(), 0.5, "distance 1 scores 0.5");
        assert_eq!(score_from_l2(3.0).unwrap(), 0.25, "distance 3 scores 0.25");
        assert!(
            score_from_l2(-0.1).is_err(),
            "negative distance is an error"
        );
        assert!(
            score_from_l2(f32::NAN).is_err(),
            "non-finite distance is an error"
        );
    }

    #[test]
    fn blank_text_and_query_are_rejected() {
        assert!(
            matches!(Text::new("  "), Err(BrainError::EmptyText)),
            "blank text is empty"
        );
        assert!(
            matches!(Query::new("\n"), Err(BrainError::EmptyQuery)),
            "blank query is empty"
        );
        assert_eq!(Text::new("  hello").unwrap().as_str(), "  hello");
    }

    #[test]
    fn limit_defaults_and_rejects_bounds() {
        assert_eq!(Limit::new(None).unwrap().get(), 5, "omitted limit is 5");
        assert_eq!(Limit::new(Some(1)).unwrap().get(), 1);
        assert_eq!(Limit::new(Some(100)).unwrap().get(), 100);
        assert!(
            matches!(Limit::new(Some(0)), Err(BrainError::BadLimit)),
            "limit 0 is rejected"
        );
        assert!(
            matches!(Limit::new(Some(101)), Err(BrainError::BadLimit)),
            "limit 101 is rejected"
        );
    }

    #[test]
    fn metadata_must_be_a_small_object() {
        let empty = Metadata::from_json(None).unwrap();
        assert_eq!(empty.as_value(), &serde_json::json!({}));
        assert!(
            matches!(
                Metadata::from_json(Some(serde_json::json!([1]))),
                Err(BrainError::MetadataNotObject)
            ),
            "an array is not metadata"
        );
        let huge = "a".repeat(16 * 1024);
        assert!(
            matches!(
                Metadata::from_json(Some(serde_json::json!({ "k": huge }))),
                Err(BrainError::MetadataTooLarge)
            ),
            "metadata over 16 KiB is rejected"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn add_twice_inserts_two_rows() {
        let dir = TempDir::new();
        let brain = Brain::open_with(dir.path(), fake(&[(PASTA, [1.0, 0.0, 0.0, 0.0])]))
            .await
            .unwrap();
        let text = Text::new(PASTA).unwrap();
        let metadata = Metadata::from_json(None).unwrap();
        let first = brain.add(text.clone(), metadata.clone()).await.unwrap();
        let second = brain.add(text, metadata).await.unwrap();
        assert_ne!(first, second, "a second add inserts a new row");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn matching_vector_ranks_first_with_score_one() {
        let dir = TempDir::new();
        let brain = Brain::open_with(dir.path(), fake(&ranked_pairs()))
            .await
            .unwrap();
        let metadata = Metadata::from_json(None).unwrap();
        let pasta_id = brain
            .add(Text::new(PASTA).unwrap(), metadata.clone())
            .await
            .unwrap();
        brain.add(Text::new(TAX).unwrap(), metadata).await.unwrap();
        let hits = brain
            .search(Query::new(QUERY).unwrap(), Limit::new(None).unwrap())
            .await
            .unwrap();
        assert_eq!(hits[0].id, pasta_id, "the matching vector ranks first");
        assert_eq!(hits[0].score, 1.0, "distance 0 scores 1");
        assert!(
            hits[0].score > hits[1].score,
            "the closer note scores higher"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn restart_keeps_id_text_and_metadata() {
        let dir = TempDir::new();
        let metadata =
            Metadata::from_json(Some(serde_json::json!({"project": "personal"}))).unwrap();
        let id = {
            let brain = Brain::open_with(dir.path(), fake(&ranked_pairs()))
                .await
                .unwrap();
            brain
                .add(Text::new(PASTA).unwrap(), metadata)
                .await
                .unwrap()
        };
        let brain = Brain::open_with(dir.path(), fake(&ranked_pairs()))
            .await
            .unwrap();
        let hits = brain
            .search(Query::new(QUERY).unwrap(), Limit::new(None).unwrap())
            .await
            .unwrap();
        assert_eq!(hits.len(), 1, "restart returns the one stored note");
        assert_eq!(hits[0].id, id, "restart keeps the id");
        assert_eq!(hits[0].text, PASTA, "restart keeps the text");
        assert_eq!(
            hits[0].metadata,
            serde_json::json!({"project": "personal"}),
            "restart keeps metadata"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_second_directory_sees_nothing() {
        let personal = TempDir::new();
        let company = TempDir::new();
        let personal_brain = Brain::open_with(personal.path(), fake(&ranked_pairs()))
            .await
            .unwrap();
        personal_brain
            .add(
                Text::new(PASTA).unwrap(),
                Metadata::from_json(None).unwrap(),
            )
            .await
            .unwrap();
        let company_brain = Brain::open_with(company.path(), fake(&ranked_pairs()))
            .await
            .unwrap();
        let hits = company_brain
            .search(Query::new(QUERY).unwrap(), Limit::new(None).unwrap())
            .await
            .unwrap();
        assert!(
            hits.is_empty(),
            "a second data directory sees no notes, got {hits:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn second_open_while_the_first_lives_is_busy() {
        let dir = TempDir::new();
        let brain = Brain::open_with(dir.path(), fake(&[])).await.unwrap();
        let err = match Brain::open_with(dir.path(), fake(&[])).await {
            Err(err) => err,
            Ok(_) => panic!("second open of a live brain should be busy"),
        };
        assert!(
            matches!(err, BrainError::WriterBusy),
            "second open of a live brain is busy, got {err}"
        );
        drop(brain);
        Brain::open_with(dir.path(), fake(&[]))
            .await
            .expect("open after the first brain drops");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn schema_version_two_does_not_create_lance() {
        let dir = TempDir::new();
        std::fs::write(
            dir.path().join("brain.toml"),
            "schema_version = 2\nmodel = \"fake\"\n",
        )
        .unwrap();
        let err = match Brain::open_with(dir.path(), fake(&[])).await {
            Err(err) => err,
            Ok(_) => panic!("schema 2 should be rejected"),
        };
        assert!(
            matches!(err, BrainError::UnsupportedSchema(2)),
            "schema 2 is rejected, got {err}"
        );
        assert!(
            !dir.path().join("lance").exists(),
            "schema 2 does not create lance/"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn model_mismatch_does_not_rewrite_toml_or_create_lance() {
        let dir = TempDir::new();
        let original = "schema_version = 1\nmodel = \"other\"\n";
        std::fs::write(dir.path().join("brain.toml"), original).unwrap();
        let err = match Brain::open_with(dir.path(), fake(&[])).await {
            Err(err) => err,
            Ok(_) => panic!("model mismatch should be rejected"),
        };
        assert!(
            matches!(err, BrainError::ModelMismatch { .. }),
            "model mismatch is rejected, got {err}"
        );
        let body = std::fs::read_to_string(dir.path().join("brain.toml")).unwrap();
        assert_eq!(body, original, "model mismatch does not rewrite brain.toml");
        assert!(
            !dir.path().join("lance").exists(),
            "model mismatch does not create lance/"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn lance_without_toml_is_corrupt() {
        let dir = TempDir::new();
        std::fs::create_dir_all(dir.path().join("lance")).unwrap();
        let err = match Brain::open_with(dir.path(), fake(&[])).await {
            Err(err) => err,
            Ok(_) => panic!("lance without brain.toml should be corrupt"),
        };
        assert!(
            matches!(err, BrainError::CorruptLayout(_)),
            "lance without brain.toml is corrupt, got {err}"
        );
    }
}
