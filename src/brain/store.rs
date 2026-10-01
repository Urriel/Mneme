use std::sync::Arc;

use arrow_array::{
    Array, ArrayRef, FixedSizeListArray, Float32Array, Float64Array, RecordBatch, StringArray,
    TimestampMicrosecondArray,
};
use arrow_schema::{DataType, Field, Schema, SchemaRef, TimeUnit};
use futures::TryStreamExt;
use lancedb::Table;
use lancedb::query::{ExecutableQuery, QueryBase, Select};

use super::embed::Vector;
use super::{BrainError, MemoryId};

const TABLE: &str = "memories";

pub(crate) struct ScoredRow {
    pub id: MemoryId,
    pub text: String,
    pub metadata: serde_json::Value,
    pub distance: f32,
}

pub(crate) struct Store {
    table: Table,
    dimensions: usize,
    schema: SchemaRef,
}

impl Store {
    pub(crate) async fn open(
        data_dir: &std::path::Path,
        dimensions: usize,
    ) -> Result<Self, BrainError> {
        let schema = memory_schema(dimensions)?;
        let lance_dir = data_dir.join("lance");
        std::fs::create_dir_all(&lance_dir)?;
        let uri = lance_dir
            .to_str()
            .ok_or_else(|| BrainError::Store("data path is not utf-8".into()))?;
        let connection = lancedb::connect(uri).execute().await.map_err(store_err)?;
        let names = connection
            .table_names()
            .execute()
            .await
            .map_err(store_err)?;
        let table = if names.iter().any(|name| name == TABLE) {
            let table = connection
                .open_table(TABLE)
                .execute()
                .await
                .map_err(store_err)?;
            let existing = table.schema().await.map_err(store_err)?;
            let width = embedding_width(&existing)?;
            if width != dimensions {
                return Err(BrainError::DimensionMismatch {
                    expected: dimensions,
                    found: width,
                });
            }
            table
        } else {
            connection
                .create_empty_table(TABLE, Arc::clone(&schema))
                .execute()
                .await
                .map_err(store_err)?
        };
        Ok(Self {
            table,
            dimensions,
            schema,
        })
    }

    pub(crate) async fn insert(
        &self,
        id: MemoryId,
        text: &str,
        vector: &Vector,
        metadata: &serde_json::Value,
    ) -> Result<(), BrainError> {
        if vector.len() != self.dimensions {
            return Err(BrainError::DimensionMismatch {
                expected: self.dimensions,
                found: vector.len(),
            });
        }
        let metadata = serde_json::to_string(metadata)
            .map_err(|err| BrainError::Store(format!("metadata did not serialize: {err}")))?;
        let batch = row_batch(&self.schema, &id.to_string(), text, vector, &metadata)?;
        self.table.add(batch).execute().await.map_err(store_err)?;
        Ok(())
    }

    pub(crate) async fn search(
        &self,
        vector: &Vector,
        limit: usize,
    ) -> Result<Vec<ScoredRow>, BrainError> {
        if vector.len() != self.dimensions {
            return Err(BrainError::DimensionMismatch {
                expected: self.dimensions,
                found: vector.len(),
            });
        }
        if self.table.count_rows(None).await.map_err(store_err)? == 0 {
            return Ok(Vec::new());
        }
        let stream = self
            .table
            .query()
            .nearest_to(vector.to_vec())
            .map_err(store_err)?
            .limit(limit)
            .select(Select::columns(&["id", "text", "metadata", "_distance"]))
            .execute()
            .await
            .map_err(store_err)?;
        let batches = stream
            .try_collect::<Vec<RecordBatch>>()
            .await
            .map_err(store_err)?;
        let mut rows = Vec::new();
        for batch in batches {
            rows.extend(rows_from_batch(&batch)?);
        }
        Ok(rows)
    }
}

fn store_err(err: impl std::fmt::Display) -> BrainError {
    BrainError::Store(err.to_string())
}

fn memory_schema(dimensions: usize) -> Result<SchemaRef, BrainError> {
    let dimensions = i32::try_from(dimensions).map_err(|_| {
        BrainError::Store(format!("embedding width {dimensions} does not fit in i32"))
    })?;
    let item = Arc::new(Field::new("item", DataType::Float32, false));
    let timestamp = DataType::Timestamp(TimeUnit::Microsecond, Some(Arc::<str>::from("UTC")));
    let fields = vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("text", DataType::Utf8, false),
        Field::new(
            "embedding",
            DataType::FixedSizeList(item, dimensions),
            false,
        ),
        Field::new("metadata", DataType::Utf8, false),
        Field::new("created_at", timestamp.clone(), false),
        Field::new("updated_at", timestamp, false),
    ];
    Ok(Arc::new(Schema::new(fields)))
}

fn embedding_width(schema: &Schema) -> Result<usize, BrainError> {
    let field = schema
        .field_with_name("embedding")
        .map_err(|err| BrainError::Store(err.to_string()))?;
    match field.data_type() {
        DataType::FixedSizeList(item, size)
            if item.data_type() == &DataType::Float32 && *size >= 0 =>
        {
            Ok(*size as usize)
        }
        other => Err(BrainError::Store(format!("embedding column is {other}"))),
    }
}

fn row_batch(
    schema: &SchemaRef,
    id: &str,
    text: &str,
    vector: &Vector,
    metadata: &str,
) -> Result<RecordBatch, BrainError> {
    let DataType::FixedSizeList(item, size) = schema
        .field_with_name("embedding")
        .map_err(|err| BrainError::Store(err.to_string()))?
        .data_type()
        .clone()
    else {
        return Err(BrainError::Store(
            "embedding column is not a fixed-size list".into(),
        ));
    };
    let values = Float32Array::from(vector.to_vec());
    let embedding = FixedSizeListArray::try_new(item, size, Arc::new(values), None)
        .map_err(|err| BrainError::Store(err.to_string()))?;
    let now = chrono::Utc::now().timestamp_micros();
    let created = TimestampMicrosecondArray::from(vec![now]).with_timezone("UTC");
    let updated = TimestampMicrosecondArray::from(vec![now]).with_timezone("UTC");
    RecordBatch::try_new(
        Arc::clone(schema),
        vec![
            Arc::new(StringArray::from(vec![id])),
            Arc::new(StringArray::from(vec![text])),
            Arc::new(embedding),
            Arc::new(StringArray::from(vec![metadata])),
            Arc::new(created),
            Arc::new(updated),
        ],
    )
    .map_err(|err| BrainError::Store(err.to_string()))
}

fn rows_from_batch(batch: &RecordBatch) -> Result<Vec<ScoredRow>, BrainError> {
    let ids = utf8_values(batch, "id")?;
    let texts = utf8_values(batch, "text")?;
    let metadata = utf8_values(batch, "metadata")?;
    let distance = batch.column_by_name("_distance").ok_or_else(|| {
        let columns = batch
            .schema()
            .fields()
            .iter()
            .map(|field| field.name().as_str())
            .collect::<Vec<_>>()
            .join(", ");
        BrainError::Store(format!("search result columns are {columns}"))
    })?;
    let mut rows = Vec::with_capacity(batch.num_rows());
    for index in 0..batch.num_rows() {
        let parsed = serde_json::from_str::<serde_json::Value>(&metadata[index])
            .map_err(|err| BrainError::Store(format!("stored metadata is not json: {err}")))?;
        if !parsed.is_object() {
            return Err(BrainError::Store("stored metadata is not an object".into()));
        }
        rows.push(ScoredRow {
            id: MemoryId::parse(&ids[index])?,
            text: texts[index].clone(),
            metadata: parsed,
            distance: distance_at(distance, index)?,
        });
    }
    Ok(rows)
}

fn utf8_values(batch: &RecordBatch, name: &str) -> Result<Vec<String>, BrainError> {
    let column = batch
        .column_by_name(name)
        .ok_or_else(|| BrainError::Store(format!("search result has no {name} column")))?;
    let casted = arrow_cast::cast(column.as_ref(), &DataType::Utf8)
        .map_err(|err| BrainError::Store(format!("{name} is not utf8: {err}")))?;
    let array = casted
        .as_any()
        .downcast_ref::<StringArray>()
        .ok_or_else(|| BrainError::Store(format!("{name} cast did not produce utf8")))?;
    let mut values = Vec::with_capacity(array.len());
    for index in 0..array.len() {
        if array.is_null(index) {
            return Err(BrainError::Store(format!("{name} is null")));
        }
        values.push(array.value(index).to_owned());
    }
    Ok(values)
}

fn distance_at(column: &ArrayRef, index: usize) -> Result<f32, BrainError> {
    if column.is_null(index) {
        return Err(BrainError::BadDistance);
    }
    if let Some(array) = column.as_any().downcast_ref::<Float32Array>() {
        return Ok(array.value(index));
    }
    if let Some(array) = column.as_any().downcast_ref::<Float64Array>() {
        return Ok(array.value(index) as f32);
    }
    Err(BrainError::Store(format!(
        "distance column is {}",
        column.data_type()
    )))
}
