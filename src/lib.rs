use std::collections::HashSet;
use std::fs::{File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};

use chrono::{DateTime, Utc};
use rmcp::ServiceExt;
use serde_json::Value;

use crate::embed::{Embedder, HashEmbedder, QwenEmbedder};
use crate::ingest::ingest_text;
use crate::search::search;
use crate::store::{Meta, Row, Store};

mod config;
mod embed;
mod ingest;
mod mcp;
mod search;
mod setup;
mod store;

pub use config::{Config, DEFAULT_DATA_DIR, DEFAULT_DEVICE, DEFAULT_DIM, DEFAULT_MODEL};
pub use search::Hit;
pub use setup::{McpInstall, SetupScope, mcp_snippet, setup};

static OPEN_DIRS: LazyLock<Mutex<HashSet<PathBuf>>> = LazyLock::new(|| Mutex::new(HashSet::new()));

#[derive(Debug)]
pub enum Error {
    ModelMismatch {
        found_model: String,
        found_dim: i32,
        asked_model: String,
        asked_dim: usize,
        data_dir: PathBuf,
    },
    DimMismatch {
        expected: usize,
        found: usize,
    },
    EmptyText,
    BadTime,
    NotFound,
    NotANote,
    CursorMovedBack,
    NotesBrain,
    BadId,
    WriterBusy,
    Device(String),
    Store(String),
    Embed(String),
    Io(std::io::Error),
}

impl std::fmt::Display for Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ModelMismatch {
                found_model,
                found_dim,
                asked_model,
                asked_dim,
                data_dir,
            } => write!(
                formatter,
                "directory {} is locked to model {found_model} dim {found_dim}, config asks for {asked_model} dim {asked_dim}. Run `mneme reembed --data {}`",
                data_dir.display(),
                data_dir.display()
            ),
            Self::DimMismatch { expected, found } => {
                write!(formatter, "embedding width is {found}, expected {expected}")
            }
            Self::EmptyText => write!(formatter, "text is empty"),
            Self::BadTime => write!(formatter, "time must be RFC3339"),
            Self::NotFound => write!(formatter, "row not found"),
            Self::NotANote => write!(formatter, "link target is not a note"),
            Self::CursorMovedBack => write!(formatter, "dream cursor cannot move backward"),
            Self::NotesBrain => write!(
                formatter,
                "directory is a notes brain. Point --data at a new folder"
            ),
            Self::BadId => write!(formatter, "id is not a ulid"),
            Self::WriterBusy => write!(formatter, "another process owns this directory"),
            Self::Device(device) => write!(formatter, "device {device} is not built. Use cpu"),
            Self::Store(message) | Self::Embed(message) => write!(formatter, "{message}"),
            Self::Io(err) => write!(formatter, "{err}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err)
    }
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
    store: Store,
    embedder: Box<dyn Embedder>,
}

struct Shared {
    state: tokio::sync::Mutex<State>,
    _guard: OpenGuard,
}

#[derive(Clone)]
pub struct Mneme {
    shared: Arc<Shared>,
}

impl Mneme {
    pub async fn init(config: &Config) -> Result<(), Error> {
        let _locked = prepare(config).await?;
        Ok(())
    }

    pub async fn reembed_message(config: &Config) -> Result<String, Error> {
        let prepared = prepare(config).await?;
        let meta = prepared.store.read_meta().await?;
        let Some(meta) = meta else {
            return Ok(format!(
                "mneme reembed is not implemented. {} has no model lock yet.",
                prepared.guard.path.display()
            ));
        };
        Ok(format!(
            "mneme reembed is not implemented. {} is locked to model {} dim {}.",
            prepared.guard.path.display(),
            meta.model,
            meta.dim
        ))
    }

    pub async fn open(config: &Config) -> Result<Self, Error> {
        let prepared = prepare(config).await?;
        let meta = required_meta(&prepared.store, config, &prepared.guard.path).await?;
        let embedder: Box<dyn Embedder> =
            if std::env::var("MNEME_EMBEDDER").ok().as_deref() == Some("fake") {
                Box::new(HashEmbedder::new(config.dim))
            } else {
                Box::new(QwenEmbedder::load(&meta.model, config.dim)?)
            };
        finish(prepared, embedder)
    }

    pub(crate) async fn open_with(
        config: &Config,
        embedder: Box<dyn Embedder>,
    ) -> Result<Self, Error> {
        if embedder.dim() != config.dim {
            return Err(Error::DimMismatch {
                expected: config.dim,
                found: embedder.dim(),
            });
        }
        let prepared = prepare(config).await?;
        let _meta = required_meta(&prepared.store, config, &prepared.guard.path).await?;
        finish(prepared, embedder)
    }

    pub async fn ingest_text(
        &self,
        text: &str,
        title: &str,
        author: &str,
    ) -> Result<String, Error> {
        self.ingest(text, title, author, None, now_micros(), now_micros())
            .await
    }

    pub async fn ingest_path(
        &self,
        path: &Path,
        title: &str,
        author: &str,
    ) -> Result<String, Error> {
        let (text, source_time) = ingest::read_file(path)?;
        let title = if title.is_empty() {
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("")
                .to_owned()
        } else {
            title.to_owned()
        };
        let path_text = path.to_string_lossy().to_string();
        self.ingest(
            &text,
            &title,
            author,
            Some(&path_text),
            now_micros(),
            source_time,
        )
        .await
    }

    async fn ingest(
        &self,
        text: &str,
        title: &str,
        author: &str,
        path: Option<&str>,
        now: i64,
        source_time: i64,
    ) -> Result<String, Error> {
        let mut state = self.shared.state.lock().await;
        let State { store, embedder } = &mut *state;
        let ingested = ingest_text(
            store,
            embedder.as_mut(),
            text,
            title,
            author,
            path,
            now,
            source_time,
        )
        .await?;
        Ok(ingested.doc_id)
    }

    pub async fn read(&self, id: &str) -> Result<Document, Error> {
        require_ulid(id)?;
        let state = self.shared.state.lock().await;
        let row = state.store.read_id(id).await?.ok_or(Error::NotFound)?;
        Ok(Document::from_row(row))
    }

    pub async fn search(
        &self,
        query: &str,
        k: usize,
        as_of: Option<u64>,
        include_superseded: bool,
    ) -> Result<Vec<Hit>, Error> {
        if k == 0 || k > 40 {
            return Err(Error::Store("k must be from 1 to 40".into()));
        }
        let mut state = self.shared.state.lock().await;
        if let Some(version) = as_of {
            state.store.checkout(version).await?;
        }
        let State { store, embedder } = &mut *state;
        let result = search(store, embedder.as_mut(), query, k, include_superseded).await;
        if as_of.is_some() {
            let restored = store.checkout_latest().await;
            if result.is_ok() {
                restored?;
            }
        }
        result
    }

    pub async fn list_recent(&self, since: Option<&str>) -> Result<Vec<Recent>, Error> {
        let state = self.shared.state.lock().await;
        let cutoff = match since {
            Some(text) => parse_time(text)?,
            None => state
                .store
                .read_meta()
                .await?
                .and_then(|meta| meta.dreamed_until)
                .unwrap_or(0),
        };
        let mut rows = state.store.recent().await?;
        rows.retain(|row| row.ingested_at > cutoff);
        rows.sort_by_key(|row| row.ingested_at);
        Ok(rows.into_iter().map(Recent::from_row).collect())
    }

    pub async fn add_note(
        &self,
        text: &str,
        source_doc_id: &str,
        links: &[String],
        supersedes: Option<&str>,
    ) -> Result<String, Error> {
        if text.trim().is_empty() {
            return Err(Error::EmptyText);
        }
        require_ulid(source_doc_id)?;
        if let Some(id) = supersedes {
            require_ulid(id)?;
        }
        for id in links {
            require_ulid(id)?;
        }
        let mut state = self.shared.state.lock().await;
        let source = state.store.read_id(source_doc_id).await?;
        if source.is_none() {
            return Err(Error::NotFound);
        }
        let meta = state
            .store
            .read_meta()
            .await?
            .ok_or_else(|| Error::Store("directory has no model lock".into()))?;
        let vectors = tokio::task::block_in_place(|| state.embedder.embed(&[text.to_owned()]))?;
        let vector = vectors
            .into_iter()
            .next()
            .ok_or_else(|| Error::Embed("embedder returned no vector".into()))?;
        let id = ulid::Ulid::new().to_string();
        let now = now_micros();
        let links_json = if links.is_empty() {
            None
        } else {
            Some(serde_json::to_string(links).map_err(|err| Error::Store(err.to_string()))?)
        };
        state
            .store
            .insert(&[Row {
                id: id.clone(),
                kind: "note".into(),
                doc_id: Some(source_doc_id.to_owned()),
                path: None,
                title: String::new(),
                text: text.to_owned(),
                author: String::new(),
                created_at: now,
                ingested_at: now,
                supersedes: supersedes.map(str::to_owned),
                source_doc_id: Some(source_doc_id.to_owned()),
                links: links_json,
                model: meta.model,
                dim: meta.dim,
                content_hash: None,
                vector: Some(vector),
            }])
            .await?;
        Ok(id)
    }

    pub async fn link(&self, from_id: &str, to_id: &str) -> Result<(), Error> {
        require_ulid(from_id)?;
        require_ulid(to_id)?;
        let mut state = self.shared.state.lock().await;
        let row = state.store.read_id(from_id).await?.ok_or(Error::NotFound)?;
        if row.kind != "note" {
            return Err(Error::NotANote);
        }
        let target = state.store.read_id(to_id).await?.ok_or(Error::NotFound)?;
        if target.kind != "note" {
            return Err(Error::NotANote);
        }
        let mut links = parse_links(row.links.as_deref())?;
        if !links.iter().any(|id| id == to_id) {
            links.push(to_id.to_owned());
        }
        let mut updated = row;
        updated.links =
            Some(serde_json::to_string(&links).map_err(|err| Error::Store(err.to_string()))?);
        state.store.delete_id(from_id).await?;
        state.store.insert(std::slice::from_ref(&updated)).await?;
        Ok(())
    }

    pub async fn set_cursor(&self, dreamed_until: &str) -> Result<(), Error> {
        let next = parse_time(dreamed_until)?;
        let state = self.shared.state.lock().await;
        let mut meta = state
            .store
            .read_meta()
            .await?
            .ok_or_else(|| Error::Store("directory has no model lock".into()))?;
        if let Some(current) = meta.dreamed_until {
            if next < current {
                return Err(Error::CursorMovedBack);
            }
        }
        meta.dreamed_until = Some(next);
        state.store.write_meta(&meta).await
    }

    pub async fn docs_version(&self) -> Result<u64, Error> {
        let state = self.shared.state.lock().await;
        state.store.version().await
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Document {
    pub id: String,
    pub kind: String,
    pub text: String,
    pub supersedes: Option<String>,
    pub links: Value,
    pub title: String,
    pub path: String,
    pub doc_id: Option<String>,
    pub source_doc_id: Option<String>,
}

impl Document {
    fn from_row(row: Row) -> Self {
        let links = row
            .links
            .as_deref()
            .and_then(|text| serde_json::from_str(text).ok())
            .unwrap_or(Value::Null);
        Self {
            id: row.id,
            kind: row.kind,
            text: row.text,
            supersedes: row.supersedes,
            links,
            title: row.title,
            path: row.path.unwrap_or_default(),
            doc_id: row.doc_id,
            source_doc_id: row.source_doc_id,
        }
    }
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, schemars::JsonSchema)]
pub struct Recent {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub path: String,
    pub ingested_at: String,
}

impl Recent {
    fn from_row(row: Row) -> Self {
        Self {
            id: row.id,
            kind: row.kind,
            title: row.title,
            path: row.path.unwrap_or_default(),
            ingested_at: format_time(row.ingested_at),
        }
    }
}

struct Prepared {
    store: Store,
    guard: OpenGuard,
}

async fn prepare(config: &Config) -> Result<Prepared, Error> {
    if config.device != crate::DEFAULT_DEVICE {
        return Err(Error::Device(config.device.clone()));
    }
    if config.data_dir.join("brain.toml").is_file() && !config.data_dir.join("docs.lance").exists()
    {
        return Err(Error::NotesBrain);
    }
    let guard = lock_dir(&config.data_dir)?;
    let store = Store::open(&guard.path, config.dim).await?;
    if store.read_meta().await?.is_none() {
        let dim = i32::try_from(config.dim).map_err(|_| Error::Store("dim does not fit".into()))?;
        store
            .write_meta(&Meta {
                model: config.model.clone(),
                dim,
                dreamed_until: None,
            })
            .await?;
    }
    Ok(Prepared { store, guard })
}

async fn required_meta(store: &Store, config: &Config, data_dir: &Path) -> Result<Meta, Error> {
    let meta = store
        .read_meta()
        .await?
        .ok_or_else(|| Error::Store("directory has no model lock".into()))?;
    if meta.model != config.model || meta.dim as usize != config.dim {
        return Err(Error::ModelMismatch {
            found_model: meta.model,
            found_dim: meta.dim,
            asked_model: config.model.clone(),
            asked_dim: config.dim,
            data_dir: data_dir.to_path_buf(),
        });
    }
    Ok(meta)
}

fn finish(prepared: Prepared, embedder: Box<dyn Embedder>) -> Result<Mneme, Error> {
    Ok(Mneme {
        shared: Arc::new(Shared {
            state: tokio::sync::Mutex::new(State {
                store: prepared.store,
                embedder,
            }),
            _guard: prepared.guard,
        }),
    })
}

pub async fn serve(config: &Config) -> Result<(), Error> {
    let mneme = Mneme::open(config).await?;
    let running = mcp::Mcp::new(mneme)
        .serve(rmcp::transport::io::stdio())
        .await
        .map_err(|err| Error::Store(err.to_string()))?;
    running
        .waiting()
        .await
        .map_err(|err| Error::Store(err.to_string()))?;
    Ok(())
}

fn lock_dir(dir: &Path) -> Result<OpenGuard, Error> {
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
            Err(Error::WriterBusy)
        }
        Err(TryLockError::Error(err)) => {
            unclaim(&path);
            Err(err.into())
        }
    }
}

fn claim(path: &Path) -> Result<(), Error> {
    let mut open = OPEN_DIRS.lock().unwrap_or_else(|err| err.into_inner());
    if open.insert(path.to_path_buf()) {
        Ok(())
    } else {
        Err(Error::WriterBusy)
    }
}

fn unclaim(path: &Path) {
    let mut open = OPEN_DIRS.lock().unwrap_or_else(|err| err.into_inner());
    open.remove(path);
}

fn now_micros() -> i64 {
    Utc::now().timestamp_micros()
}

pub fn parse_time(text: &str) -> Result<i64, Error> {
    let parsed = DateTime::parse_from_rfc3339(text).map_err(|_| Error::BadTime)?;
    Ok(parsed.timestamp_micros())
}

fn format_time(micros: i64) -> String {
    DateTime::<Utc>::from_timestamp_micros(micros)
        .map(|time| time.to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true))
        .unwrap_or_default()
}

fn require_ulid(id: &str) -> Result<ulid::Ulid, Error> {
    ulid::Ulid::from_string(id).map_err(|_| Error::BadId)
}

fn parse_links(text: Option<&str>) -> Result<Vec<String>, Error> {
    let Some(text) = text else {
        return Ok(Vec::new());
    };
    serde_json::from_str(text).map_err(|err| Error::Store(err.to_string()))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);
    use crate::embed::MapEmbedder;

    const PASTA: &str = "Boil salted water and simmer dried pasta until tender.";
    const TAX: &str = "File the tax forms before the April deadline.";
    const QUERY: &str = "pasta";

    fn config(dir: &Path) -> Config {
        Config {
            data_dir: dir.to_path_buf(),
            model: "fake".into(),
            dim: 4,
            device: "cpu".into(),
        }
    }

    fn embedder() -> Box<dyn Embedder> {
        Box::new(MapEmbedder::new(
            4,
            &[
                (PASTA, vec![1.0, 0.0, 0.0, 0.0]),
                (TAX, vec![0.0, 1.0, 0.0, 0.0]),
                (QUERY, vec![1.0, 0.0, 0.0, 0.0]),
                ("old fact about volcano ash", vec![0.0, 0.0, 1.0, 0.0]),
                ("new fact about roman concrete", vec![0.0, 0.0, 0.0, 1.0]),
                ("volcano", vec![0.0, 0.0, 1.0, 0.0]),
                ("concrete", vec![0.0, 0.0, 0.0, 1.0]),
            ],
        ))
    }

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir()
                .join(format!("mneme-docs-{stamp}-{seq}-{}", std::process::id()));
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

    #[tokio::test(flavor = "multi_thread")]
    async fn ingest_round_trip_is_idempotent() {
        let dir = TempDir::new();
        let brain = Mneme::open_with(&config(dir.path()), embedder())
            .await
            .unwrap();
        let first = brain.ingest_text(PASTA, "Pasta", "Ada").await.unwrap();
        let second = brain.ingest_text(PASTA, "Pasta", "Ada").await.unwrap();
        assert_eq!(first, second, "the same text returns the same doc id");
        let doc = brain.read(&first).await.unwrap();
        assert_eq!(doc.kind, "doc");
        assert_eq!(doc.text, PASTA, "read returns the full document");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn search_returns_the_document() {
        let dir = TempDir::new();
        let brain = Mneme::open_with(&config(dir.path()), embedder())
            .await
            .unwrap();
        let pasta = brain.ingest_text(PASTA, "Pasta", "").await.unwrap();
        brain.ingest_text(TAX, "Tax", "").await.unwrap();
        let hits = brain.search(QUERY, 5, None, false).await.unwrap();
        assert_eq!(
            hits[0].doc_id, pasta,
            "search returns the pasta doc id: {hits:?}"
        );
        assert!(
            hits[0].excerpt.contains("pasta"),
            "excerpt comes from the chunk"
        );
        let doc = brain.read(&hits[0].doc_id).await.unwrap();
        assert_eq!(doc.text, PASTA, "read returns the full text after search");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_replacement_file_hides_the_old_doc_until_as_of() {
        let dir = TempDir::new();
        let file = dir.path().join("note.txt");
        std::fs::write(&file, "old fact about volcano ash").unwrap();
        let brain = Mneme::open_with(&config(dir.path()), embedder())
            .await
            .unwrap();
        let old_id = brain.ingest_path(&file, "", "").await.unwrap();
        let version = brain.docs_version().await.unwrap();
        std::fs::write(&file, "new fact about roman concrete").unwrap();
        let new_id = brain.ingest_path(&file, "", "").await.unwrap();
        assert_ne!(old_id, new_id, "a changed file writes a new doc");
        let current = brain.search("concrete", 5, None, false).await.unwrap();
        assert_eq!(
            current[0].doc_id, new_id,
            "current search returns the new doc"
        );
        let old_hits = brain.search("volcano", 5, None, false).await.unwrap();
        assert!(
            old_hits.iter().all(|hit| hit.doc_id != old_id),
            "the superseded doc stays out of current search: {old_hits:?}"
        );
        let included = brain.search("volcano", 5, None, true).await.unwrap();
        assert!(
            included.iter().any(|hit| hit.doc_id == old_id),
            "include_superseded returns the old doc: {included:?}"
        );
        let past = brain
            .search("volcano", 5, Some(version), false)
            .await
            .unwrap();
        assert_eq!(
            past[0].doc_id, old_id,
            "as_of returns the doc from that version"
        );
        let after = brain.search("concrete", 5, None, false).await.unwrap();
        assert_eq!(
            after[0].doc_id, new_id,
            "search returns to latest after as_of"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn cursor_notes_and_links() {
        let dir = TempDir::new();
        let brain = Mneme::open_with(&config(dir.path()), embedder())
            .await
            .unwrap();
        let doc_id = brain.ingest_text(PASTA, "Pasta", "").await.unwrap();
        let recent = brain.list_recent(None).await.unwrap();
        assert_eq!(recent.len(), 1, "a null cursor lists the ingested doc");
        let cursor = recent[0].ingested_at.clone();
        brain.set_cursor(&cursor).await.unwrap();
        assert!(
            brain.list_recent(None).await.unwrap().is_empty(),
            "list_recent stops at the cursor"
        );
        let note = brain
            .add_note(
                "Salt the water before the pasta goes in.",
                &doc_id,
                &[],
                None,
            )
            .await
            .unwrap();
        let other = brain
            .add_note("Drain the pasta when it is tender.", &doc_id, &[], None)
            .await
            .unwrap();
        brain.link(&note, &other).await.unwrap();
        let loaded = brain.read(&note).await.unwrap();
        assert_eq!(loaded.kind, "note");
        assert_eq!(
            loaded.links,
            serde_json::json!([other]),
            "link appends the note id"
        );
        let listed = brain.list_recent(None).await.unwrap();
        assert!(
            listed.iter().any(|row| row.id == note),
            "notes newer than the cursor are listed: {listed:?}"
        );
        let err = match brain.set_cursor("2000-01-01T00:00:00Z").await {
            Err(err) => err,
            Ok(()) => panic!("an older cursor should be rejected"),
        };
        assert!(matches!(err, Error::CursorMovedBack));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_notes_brain_is_refused() {
        let dir = TempDir::new();
        std::fs::write(
            dir.path().join("brain.toml"),
            "schema_version = 1\nmodel = \"fake\"\n",
        )
        .unwrap();
        let err = match Mneme::open_with(&config(dir.path()), embedder()).await {
            Err(err) => err,
            Ok(_) => panic!("a notes brain should be refused"),
        };
        assert!(matches!(err, Error::NotesBrain), "got {err}");
        assert!(
            !dir.path().join("docs.lance").exists(),
            "refusal creates no docs table"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn model_mismatch_names_reembed() {
        let dir = TempDir::new();
        Mneme::init(&config(dir.path())).await.unwrap();
        let mut other = config(dir.path());
        other.model = "other".into();
        let err = match Mneme::open_with(&other, embedder()).await {
            Err(err) => err,
            Ok(_) => panic!("a different model should be refused"),
        };
        let message = err.to_string();
        assert!(
            message.contains("mneme reembed"),
            "mismatch tells the operator to reembed: {message}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn qwen_width_matches_the_default_dim() {
        let embedder = crate::embed::QwenEmbedder::load(DEFAULT_MODEL, DEFAULT_DIM).unwrap();
        assert_eq!(
            embedder.dim(),
            DEFAULT_DIM,
            "Qwen3-Embedding-0.6B is 1024 wide"
        );
    }

    #[test]
    fn chunks_overlap_on_paragraph_boundaries() {
        let paragraph = "word ".repeat(500);
        let text = format!("{paragraph}\n\n{paragraph}");
        let chunks = ingest::chunk_text(&text);
        assert!(
            chunks.len() >= 2,
            "a long document splits, got {}",
            chunks.len()
        );
        let first_words: Vec<_> = chunks[0].split_whitespace().collect();
        let second_words: Vec<_> = chunks[1].split_whitespace().collect();
        assert!(
            second_words.starts_with(&first_words[first_words.len().saturating_sub(100)..]),
            "the next chunk overlaps the previous tail"
        );
    }
}
