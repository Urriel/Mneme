use std::sync::Arc;

use arrow_array::{
    Array, ArrayRef, FixedSizeListArray, Float32Array, Int32Array, RecordBatch, StringArray,
    TimestampMicrosecondArray,
};
use arrow_buffer::NullBuffer;
use arrow_cast::cast;
use arrow_schema::{DataType, Field, Schema, SchemaRef, TimeUnit};
use futures::TryStreamExt;
use lancedb::Table;
use lancedb::index::Index;
use lancedb::index::scalar::FullTextSearchQuery;
use lancedb::query::{ExecutableQuery, QueryBase, Select};

use crate::Error;

pub(crate) const DOCS: &str = "docs";
pub(crate) const META: &str = "meta";

#[derive(Clone, Debug)]
pub(crate) struct Meta {
    pub model: String,
    pub dim: i32,
    pub dreamed_until: Option<i64>,
}

#[derive(Clone, Debug)]
pub(crate) struct Row {
    pub id: String,
    pub kind: String,
    pub doc_id: Option<String>,
    pub path: Option<String>,
    pub title: String,
    pub text: String,
    pub author: String,
    pub created_at: i64,
    pub ingested_at: i64,
    pub supersedes: Option<String>,
    pub source_doc_id: Option<String>,
    pub links: Option<String>,
    pub model: String,
    pub dim: i32,
    pub content_hash: Option<String>,
    pub vector: Option<Vec<f32>>,
}

#[derive(Clone, Debug)]
pub(crate) struct ChunkHit {
    pub id: String,
    pub doc_id: String,
    pub text: String,
}

pub(crate) struct Store {
    docs: Table,
    meta: Table,
    schema: SchemaRef,
    dim: usize,
    fts_version: Option<u64>,
    pinned: bool,
}

impl Store {
    pub(crate) async fn open(data_dir: &std::path::Path, dim: usize) -> Result<Self, Error> {
        let schema = docs_schema(dim)?;
        let uri = data_dir
            .to_str()
            .ok_or_else(|| Error::Store("data path is not utf-8".into()))?;
        let connection = lancedb::connect(uri).execute().await.map_err(store_err)?;
        let docs = open_or_create(&connection, DOCS, Arc::clone(&schema), dim).await?;
        let meta = open_or_create(&connection, META, meta_schema(), 0).await?;
        Ok(Self {
            docs,
            meta,
            schema,
            dim,
            fts_version: None,
            pinned: false,
        })
    }

    pub(crate) async fn read_meta(&self) -> Result<Option<Meta>, Error> {
        let rows = collect(&self.meta, None, 10).await?;
        let mut found = None;
        for batch in rows {
            let models = utf8(&batch, "model")?;
            let dims = int32(&batch, "dim")?;
            let until = timestamps(&batch, "dreamed_until")?;
            for index in 0..batch.num_rows() {
                found = Some(Meta {
                    model: models[index].clone().unwrap_or_default(),
                    dim: dims[index],
                    dreamed_until: until[index],
                });
            }
        }
        Ok(found)
    }

    pub(crate) async fn write_meta(&self, meta: &Meta) -> Result<(), Error> {
        if self.meta.count_rows(None).await.map_err(store_err)? > 0 {
            self.meta
                .delete("model IS NOT NULL")
                .await
                .map_err(store_err)?;
        }
        let schema = meta_schema();
        let model = StringArray::from(vec![meta.model.clone()]);
        let dim = Int32Array::from(vec![meta.dim]);
        let until = TimestampMicrosecondArray::from(vec![meta.dreamed_until]).with_timezone("UTC");
        let batch = RecordBatch::try_new(
            schema,
            vec![Arc::new(model), Arc::new(dim), Arc::new(until)],
        )
        .map_err(store_err)?;
        self.meta.add(batch).execute().await.map_err(store_err)?;
        Ok(())
    }

    pub(crate) async fn insert(&mut self, rows: &[Row]) -> Result<(), Error> {
        if rows.is_empty() {
            return Ok(());
        }
        let batch = rows_batch(&self.schema, self.dim, rows)?;
        self.docs.add(batch).execute().await.map_err(store_err)?;
        self.fts_version = None;
        Ok(())
    }

    pub(crate) async fn delete_id(&mut self, id: &str) -> Result<(), Error> {
        let predicate = format!("id = '{id}'");
        self.docs.delete(&predicate).await.map_err(store_err)?;
        self.fts_version = None;
        Ok(())
    }

    pub(crate) async fn read_id(&self, id: &str) -> Result<Option<Row>, Error> {
        let batches = collect(&self.docs, Some(&format!("id = '{id}'")), 5).await?;
        let mut rows = rows_from(&batches, self.dim)?;
        Ok(rows.pop())
    }

    pub(crate) async fn docs(&self) -> Result<Vec<Row>, Error> {
        let batches = collect(&self.docs, Some("kind = 'doc'"), 100_000).await?;
        rows_from(&batches, self.dim)
    }

    pub(crate) async fn notes(&self) -> Result<Vec<Row>, Error> {
        let batches = collect(&self.docs, Some("kind = 'note'"), 100_000).await?;
        rows_from(&batches, self.dim)
    }

    pub(crate) async fn recent(&self) -> Result<Vec<Row>, Error> {
        let batches = collect(&self.docs, Some("kind = 'doc' OR kind = 'note'"), 100_000).await?;
        rows_from(&batches, self.dim)
    }

    pub(crate) async fn version(&self) -> Result<u64, Error> {
        self.docs.version().await.map_err(store_err)
    }

    pub(crate) async fn vector_search(&mut self, vector: &[f32]) -> Result<Vec<ChunkHit>, Error> {
        self.nearest("kind = 'chunk'", vector).await
    }

    pub(crate) async fn note_vector_search(
        &mut self,
        vector: &[f32],
    ) -> Result<Vec<ChunkHit>, Error> {
        self.nearest("kind = 'note'", vector).await
    }

    pub(crate) async fn text_search(&mut self, query: &str) -> Result<Vec<ChunkHit>, Error> {
        self.words("kind = 'chunk'", query).await
    }

    pub(crate) async fn note_text_search(&mut self, query: &str) -> Result<Vec<ChunkHit>, Error> {
        self.words("kind = 'note'", query).await
    }

    async fn nearest(&mut self, filter: &str, vector: &[f32]) -> Result<Vec<ChunkHit>, Error> {
        if self
            .docs
            .count_rows(Some(filter.to_owned()))
            .await
            .map_err(store_err)?
            == 0
        {
            return Ok(Vec::new());
        }
        let stream = self
            .docs
            .query()
            .only_if(filter)
            .nearest_to(vector.to_vec())
            .map_err(store_err)?
            .limit(40)
            .select(Select::columns(&["id", "doc_id", "text"]))
            .execute()
            .await
            .map_err(store_err)?;
        let batches = stream.try_collect::<Vec<_>>().await.map_err(store_err)?;
        chunk_hits(&batches)
    }

    async fn words(&mut self, filter: &str, query: &str) -> Result<Vec<ChunkHit>, Error> {
        if self.pinned {
            return Ok(Vec::new());
        }
        if self
            .docs
            .count_rows(Some(filter.to_owned()))
            .await
            .map_err(store_err)?
            == 0
        {
            return Ok(Vec::new());
        }
        self.ensure_fts().await?;
        let stream = self
            .docs
            .query()
            .only_if(filter)
            .full_text_search(FullTextSearchQuery::new(query.to_owned()))
            .limit(40)
            .select(Select::columns(&["id", "doc_id", "text"]))
            .execute()
            .await
            .map_err(store_err)?;
        let batches = stream.try_collect::<Vec<_>>().await.map_err(store_err)?;
        chunk_hits(&batches)
    }

    pub(crate) async fn checkout(&mut self, version: u64) -> Result<(), Error> {
        self.docs.checkout(version).await.map_err(store_err)?;
        self.pinned = true;
        Ok(())
    }

    pub(crate) async fn checkout_latest(&mut self) -> Result<(), Error> {
        self.docs.checkout_latest().await.map_err(store_err)?;
        self.pinned = false;
        self.fts_version = None;
        Ok(())
    }

    async fn ensure_fts(&mut self) -> Result<(), Error> {
        let version = self.docs.version().await.map_err(store_err)?;
        if self.fts_version == Some(version) {
            return Ok(());
        }
        self.docs
            .create_index(&["text"], Index::FTS(Default::default()))
            .replace(true)
            .execute()
            .await
            .map_err(store_err)?;
        self.fts_version = Some(self.docs.version().await.map_err(store_err)?);
        Ok(())
    }
}

async fn open_or_create(
    connection: &lancedb::Connection,
    name: &str,
    schema: SchemaRef,
    vector_dim: usize,
) -> Result<Table, Error> {
    let names = connection
        .table_names()
        .execute()
        .await
        .map_err(store_err)?;
    if names.iter().any(|existing| existing == name) {
        let table = connection
            .open_table(name)
            .execute()
            .await
            .map_err(store_err)?;
        if vector_dim > 0 {
            let existing = table.schema().await.map_err(store_err)?;
            let width = vector_width(&existing)?;
            if width != vector_dim {
                return Err(Error::DimMismatch {
                    expected: vector_dim,
                    found: width,
                });
            }
        }
        return Ok(table);
    }
    connection
        .create_empty_table(name, schema)
        .execute()
        .await
        .map_err(store_err)
}

fn docs_schema(dim: usize) -> Result<SchemaRef, Error> {
    let dim = i32::try_from(dim).map_err(|_| Error::Store(format!("dim {dim} does not fit")))?;
    let item = Arc::new(Field::new("item", DataType::Float32, false));
    let time = DataType::Timestamp(TimeUnit::Microsecond, Some(Arc::<str>::from("UTC")));
    Ok(Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("kind", DataType::Utf8, false),
        Field::new("doc_id", DataType::Utf8, true),
        Field::new("path", DataType::Utf8, true),
        Field::new("title", DataType::Utf8, false),
        Field::new("text", DataType::Utf8, false),
        Field::new("vector", DataType::FixedSizeList(item, dim), true),
        Field::new("author", DataType::Utf8, false),
        Field::new("created_at", time.clone(), false),
        Field::new("ingested_at", time, false),
        Field::new("supersedes", DataType::Utf8, true),
        Field::new("source_doc_id", DataType::Utf8, true),
        Field::new("links", DataType::Utf8, true),
        Field::new("model", DataType::Utf8, false),
        Field::new("dim", DataType::Int32, false),
        Field::new("content_hash", DataType::Utf8, true),
    ])))
}

fn meta_schema() -> SchemaRef {
    let time = DataType::Timestamp(TimeUnit::Microsecond, Some(Arc::<str>::from("UTC")));
    Arc::new(Schema::new(vec![
        Field::new("model", DataType::Utf8, false),
        Field::new("dim", DataType::Int32, false),
        Field::new("dreamed_until", time, true),
    ]))
}

fn vector_width(schema: &Schema) -> Result<usize, Error> {
    let field = schema
        .field_with_name("vector")
        .map_err(|err| Error::Store(err.to_string()))?;
    match field.data_type() {
        DataType::FixedSizeList(item, size)
            if item.data_type() == &DataType::Float32 && *size > 0 =>
        {
            Ok(*size as usize)
        }
        other => Err(Error::Store(format!("vector column is {other}"))),
    }
}

fn rows_batch(schema: &SchemaRef, dim: usize, rows: &[Row]) -> Result<RecordBatch, Error> {
    let mut values = Vec::with_capacity(rows.len() * dim);
    let mut valid = Vec::with_capacity(rows.len());
    for row in rows {
        match &row.vector {
            Some(vector) if vector.len() == dim => {
                values.extend(vector.iter().copied());
                valid.push(true);
            }
            Some(vector) => {
                return Err(Error::DimMismatch {
                    expected: dim,
                    found: vector.len(),
                });
            }
            None => {
                values.extend(std::iter::repeat(0.0).take(dim));
                valid.push(false);
            }
        }
    }
    let item = Arc::new(Field::new("item", DataType::Float32, false));
    let lists = FixedSizeListArray::try_new(
        item,
        i32::try_from(dim).map_err(|_| Error::Store("dim does not fit".into()))?,
        Arc::new(Float32Array::from(values)) as ArrayRef,
        Some(NullBuffer::from(valid)),
    )
    .map_err(store_err)?;
    let batch = RecordBatch::try_new(
        Arc::clone(schema),
        vec![
            Arc::new(StringArray::from(
                rows.iter().map(|row| row.id.clone()).collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(
                rows.iter().map(|row| row.kind.clone()).collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(
                rows.iter()
                    .map(|row| row.doc_id.clone())
                    .collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(
                rows.iter().map(|row| row.path.clone()).collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(
                rows.iter().map(|row| row.title.clone()).collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(
                rows.iter().map(|row| row.text.clone()).collect::<Vec<_>>(),
            )),
            Arc::new(lists),
            Arc::new(StringArray::from(
                rows.iter()
                    .map(|row| row.author.clone())
                    .collect::<Vec<_>>(),
            )),
            Arc::new(
                TimestampMicrosecondArray::from(
                    rows.iter().map(|row| row.created_at).collect::<Vec<_>>(),
                )
                .with_timezone("UTC"),
            ),
            Arc::new(
                TimestampMicrosecondArray::from(
                    rows.iter().map(|row| row.ingested_at).collect::<Vec<_>>(),
                )
                .with_timezone("UTC"),
            ),
            Arc::new(StringArray::from(
                rows.iter()
                    .map(|row| row.supersedes.clone())
                    .collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(
                rows.iter()
                    .map(|row| row.source_doc_id.clone())
                    .collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(
                rows.iter().map(|row| row.links.clone()).collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(
                rows.iter().map(|row| row.model.clone()).collect::<Vec<_>>(),
            )),
            Arc::new(Int32Array::from(
                rows.iter().map(|row| row.dim).collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(
                rows.iter()
                    .map(|row| row.content_hash.clone())
                    .collect::<Vec<_>>(),
            )),
        ],
    )
    .map_err(store_err)?;
    Ok(batch)
}

async fn collect(
    table: &Table,
    filter: Option<&str>,
    limit: usize,
) -> Result<Vec<RecordBatch>, Error> {
    let mut query = table.query().limit(limit);
    if let Some(filter) = filter {
        query = query.only_if(filter);
    }
    let stream = query.execute().await.map_err(store_err)?;
    stream.try_collect().await.map_err(store_err)
}

fn rows_from(batches: &[RecordBatch], dim: usize) -> Result<Vec<Row>, Error> {
    let mut rows = Vec::new();
    for batch in batches {
        let ids = utf8(batch, "id")?;
        let kinds = utf8(batch, "kind")?;
        let doc_ids = optional_utf8(batch, "doc_id")?;
        let paths = optional_utf8(batch, "path")?;
        let titles = utf8(batch, "title")?;
        let texts = utf8(batch, "text")?;
        let authors = utf8(batch, "author")?;
        let created = timestamps(batch, "created_at")?;
        let ingested = timestamps(batch, "ingested_at")?;
        let supersedes = optional_utf8(batch, "supersedes")?;
        let sources = optional_utf8(batch, "source_doc_id")?;
        let links = optional_utf8(batch, "links")?;
        let models = utf8(batch, "model")?;
        let dims = int32(batch, "dim")?;
        let hashes = optional_utf8(batch, "content_hash")?;
        let vectors = vectors(batch, dim)?;
        for index in 0..batch.num_rows() {
            rows.push(Row {
                id: ids[index].clone().unwrap_or_default(),
                kind: kinds[index].clone().unwrap_or_default(),
                doc_id: doc_ids[index].clone(),
                path: paths[index].clone(),
                title: titles[index].clone().unwrap_or_default(),
                text: texts[index].clone().unwrap_or_default(),
                author: authors[index].clone().unwrap_or_default(),
                created_at: created[index].unwrap_or(0),
                ingested_at: ingested[index].unwrap_or(0),
                supersedes: supersedes[index].clone(),
                source_doc_id: sources[index].clone(),
                links: links[index].clone(),
                model: models[index].clone().unwrap_or_default(),
                dim: dims[index],
                content_hash: hashes[index].clone(),
                vector: vectors[index].clone(),
            });
        }
    }
    Ok(rows)
}

fn chunk_hits(batches: &[RecordBatch]) -> Result<Vec<ChunkHit>, Error> {
    let mut hits = Vec::new();
    for batch in batches {
        let ids = utf8(batch, "id")?;
        let doc_ids = utf8(batch, "doc_id")?;
        let texts = utf8(batch, "text")?;
        for index in 0..batch.num_rows() {
            let Some(doc_id) = doc_ids[index].clone() else {
                continue;
            };
            hits.push(ChunkHit {
                id: ids[index].clone().unwrap_or_default(),
                doc_id,
                text: texts[index].clone().unwrap_or_default(),
            });
        }
    }
    Ok(hits)
}

fn utf8(batch: &RecordBatch, name: &str) -> Result<Vec<Option<String>>, Error> {
    let array = batch
        .column_by_name(name)
        .ok_or_else(|| Error::Store(format!("missing column {name}")))?;
    let casted = cast(array, &DataType::Utf8).map_err(store_err)?;
    let values = casted
        .as_any()
        .downcast_ref::<StringArray>()
        .ok_or_else(|| Error::Store(format!("{name} is not utf8")))?;
    Ok((0..values.len())
        .map(|index| {
            if values.is_null(index) {
                None
            } else {
                Some(values.value(index).to_owned())
            }
        })
        .collect())
}

fn optional_utf8(batch: &RecordBatch, name: &str) -> Result<Vec<Option<String>>, Error> {
    if batch.column_by_name(name).is_none() {
        return Ok(vec![None; batch.num_rows()]);
    }
    utf8(batch, name)
}

fn int32(batch: &RecordBatch, name: &str) -> Result<Vec<i32>, Error> {
    let array = batch
        .column_by_name(name)
        .ok_or_else(|| Error::Store(format!("missing column {name}")))?;
    let values = array
        .as_any()
        .downcast_ref::<Int32Array>()
        .ok_or_else(|| Error::Store(format!("{name} is not int32")))?;
    Ok((0..values.len()).map(|index| values.value(index)).collect())
}

fn timestamps(batch: &RecordBatch, name: &str) -> Result<Vec<Option<i64>>, Error> {
    let array = batch
        .column_by_name(name)
        .ok_or_else(|| Error::Store(format!("missing column {name}")))?;
    let values = array
        .as_any()
        .downcast_ref::<TimestampMicrosecondArray>()
        .ok_or_else(|| Error::Store(format!("{name} is not a timestamp")))?;
    Ok((0..values.len())
        .map(|index| {
            if values.is_null(index) {
                None
            } else {
                Some(values.value(index))
            }
        })
        .collect())
}

fn vectors(batch: &RecordBatch, dim: usize) -> Result<Vec<Option<Vec<f32>>>, Error> {
    let Some(array) = batch.column_by_name("vector") else {
        return Ok(vec![None; batch.num_rows()]);
    };
    let lists = array
        .as_any()
        .downcast_ref::<FixedSizeListArray>()
        .ok_or_else(|| Error::Store("vector column is not a list".into()))?;
    let mut out = Vec::with_capacity(batch.num_rows());
    for index in 0..lists.len() {
        if lists.is_null(index) {
            out.push(None);
            continue;
        }
        let values = lists.value(index);
        let floats = values
            .as_any()
            .downcast_ref::<Float32Array>()
            .ok_or_else(|| Error::Store("vector values are not float32".into()))?;
        if floats.len() != dim {
            return Err(Error::DimMismatch {
                expected: dim,
                found: floats.len(),
            });
        }
        out.push(Some(floats.values().to_vec()));
    }
    Ok(out)
}

fn store_err(err: impl std::fmt::Display) -> Error {
    Error::Store(err.to_string())
}
