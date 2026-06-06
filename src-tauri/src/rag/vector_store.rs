//! LanceDB-backed vector store for RAG.
//!
//! Provides a wrapper around LanceDB for vector similarity search
//! over document chunks. Data is stored in an embedded LanceDB database
//! at `{agentjax_home}/rag/`.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use arrow_array::{
    Array, ArrayRef, FixedSizeListArray, Float32Array, Int32Array, RecordBatch, StringArray,
};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use futures_util::StreamExt;

use crate::error::{AgentJaxError, AgentJaxResult};
use lancedb::query::{ExecutableQuery, QueryBase};

use super::types::{Chunk, SearchConfig, SearchResult};

/// The LanceDB vector store for document chunks.
pub struct VectorStore {
    /// Path to the LanceDB database directory.
    db_path: String,
}

impl VectorStore {
    /// Open or create a vector store at the given directory path.
    pub async fn open(path: impl AsRef<Path>) -> AgentJaxResult<Self> {
        let db_path = path.as_ref().to_string_lossy().to_string();

        // Ensure the directory exists
        if let Some(parent) = path.as_ref().parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|e| {
                AgentJaxError::embedding(format!(
                    "Failed to create vector store directory {}: {e}",
                    parent.display()
                ))
            })?;
        }

        Ok(Self { db_path })
    }

    /// Insert a batch of chunks into the vector store.
    ///
    /// Each chunk must have its `embedding` field populated.
    pub async fn insert_chunks(&self, chunks: &[Chunk]) -> AgentJaxResult<()> {
        if chunks.is_empty() {
            return Ok(());
        }

        let dims = chunks
            .iter()
            .find_map(|c| c.embedding.as_ref().map(|e| e.len()))
            .unwrap_or(0);

        if dims == 0 {
            return Err(AgentJaxError::embedding(
                "Cannot insert chunks without embeddings: embedding field is empty",
            ));
        }

        let batch = self.build_record_batch(chunks, dims)?;
        let db = self.open_db().await?;
        let table = self.open_or_create_table(&db, vec![batch.clone()]).await?;

        table.add(vec![batch]).execute().await.map_err(|e| {
            AgentJaxError::embedding(format!("Failed to add to LanceDB table: {e}"))
        })?;

        Ok(())
    }

    /// Search for the nearest neighbors of a query vector.
    pub async fn search(
        &self,
        query: &[f32],
        config: &SearchConfig,
    ) -> AgentJaxResult<Vec<SearchResult>> {
        if query.is_empty() {
            return Err(AgentJaxError::embedding("Query vector is empty"));
        }

        let db = self.open_db().await?;
        let table = self.open_table(&db).await.map_err(|e| {
            e.with_context("search vector store")
        })?;

        // Build the ANN search query
        let mut query_builder = table
            .query()
            .nearest_to(query.to_vec())
            .map_err(|e| AgentJaxError::embedding(format!("Invalid query vector: {e}")))?;

        if let Some(ref filter_expr) = config.filter {
            query_builder = query_builder.only_if(filter_expr);
        }

        query_builder = query_builder.limit(config.top_k);

        // Execute and collect results
        let stream = query_builder
            .execute()
            .await
            .map_err(|e| AgentJaxError::embedding(format!("LanceDB search failed: {e}")))?;

        let batches = self.collect_stream(stream).await?;

        self.parse_results(&batches, config.min_score)
    }

    /// Delete all chunks belonging to a document.
    pub async fn delete_document(&self, document_id: &str) -> AgentJaxResult<()> {
        let db = self.open_db().await?;
        let table = self.open_table(&db).await.map_err(|e| {
            e.with_context("delete document")
        })?;

        table
            .delete(format!("document_id = '{document_id}'").as_str())
            .await
            .map_err(|e| AgentJaxError::embedding(format!("Failed to delete from LanceDB: {e}")))?;

        Ok(())
    }

    /// List all document IDs in the store.
    pub async fn list_documents(&self) -> AgentJaxResult<Vec<String>> {
        let db = self.open_db().await?;

        let table = match db.open_table("document_chunks").execute().await {
            Ok(t) => t,
            Err(_) => return Ok(vec![]), // Table doesn't exist yet
        };

        let stream = table
            .query()
            .limit(10000)
            .execute()
            .await
            .map_err(|e| AgentJaxError::embedding(format!("Failed to query LanceDB: {e}")))?;

        let batches = self.collect_stream(stream).await?;

        let mut doc_ids: Vec<String> = Vec::new();
        let mut seen = std::collections::HashSet::new();

        for batch in &batches {

            if let Some(col) = batch.column_by_name("document_id") {
                let arr = col.as_any().downcast_ref::<StringArray>().unwrap();
                for i in 0..arr.len() {
                    if arr.is_null(i) {
                        continue;
                    }
                    let val = arr.value(i);
                    if seen.insert(val.to_string()) {
                        doc_ids.push(val.to_string());
                    }
                }
            }
        }

        Ok(doc_ids)
    }

    // ── Private helpers ─────────────────────────────────────────────────────

    /// Open the LanceDB database connection.
    async fn open_db(&self) -> AgentJaxResult<lancedb::Connection> {
        lancedb::connect(&self.db_path)
            .execute()
            .await
            .map_err(|e| AgentJaxError::embedding(format!("Failed to open LanceDB: {e}")))
    }

    /// Open the document_chunks table, creating it if it doesn't exist
    /// with the given batches as schema template.
    async fn open_or_create_table(
        &self,
        db: &lancedb::Connection,
        schema_batches: Vec<RecordBatch>,
    ) -> AgentJaxResult<lancedb::Table> {
        let table_name = "document_chunks";
        match db.open_table(table_name).execute().await {
            Ok(table) => Ok(table),
            Err(_) => db
                .create_table(table_name, schema_batches)
                .execute()
                .await
                .map_err(|e| {
                    AgentJaxError::embedding(format!("Failed to create LanceDB table: {e}"))
                }),
        }
    }

    /// Open the existing document_chunks table.
    async fn open_table(&self, db: &lancedb::Connection) -> AgentJaxResult<lancedb::Table> {
        db.open_table("document_chunks")
            .execute()
            .await
            .map_err(|e| {
                AgentJaxError::embedding(format!("Failed to open LanceDB table: {e}"))
            })
    }

    /// Collect all batches from a query result stream.
    async fn collect_stream(
        &self,
        mut stream: impl futures_util::Stream<Item = Result<RecordBatch, lancedb::Error>> + std::marker::Unpin,
    ) -> AgentJaxResult<Vec<RecordBatch>> {
        let mut batches = Vec::new();
        while let Some(batch_result) = stream.next().await {
            let batch = batch_result
                .map_err(|e| AgentJaxError::embedding(format!("LanceDB stream error: {e}")))?;
            batches.push(batch);
        }
        Ok(batches)
    }

    /// Get a StringArray column from a batch, returning a descriptive error if missing.
    fn string_column<'a>(batch: &'a RecordBatch, name: &str) -> AgentJaxResult<&'a StringArray> {
        batch
            .column_by_name(name)
            .and_then(|c| c.as_any().downcast_ref::<StringArray>())
            .ok_or_else(|| {
                AgentJaxError::embedding(format!("Missing '{name}' column in search results"))
            })
    }

    fn make_schema(&self, dims: usize) -> SchemaRef {
        Schema::new(vec![
            Field::new("id", DataType::Utf8, false),
            Field::new("document_id", DataType::Utf8, false),
            Field::new("chunk_index", DataType::Int32, false),
            Field::new("content", DataType::Utf8, false),
            Field::new("metadata", DataType::Utf8, true), // JSON string
            Field::new(
                "embedding",
                DataType::FixedSizeList(
                    Arc::new(Field::new("item", DataType::Float32, true)),
                    dims as i32,
                ),
                true,
            ),
        ])
        .into()
    }

    fn build_record_batch(&self, chunks: &[Chunk], dims: usize) -> AgentJaxResult<RecordBatch> {
        let _n = chunks.len();

        let ids: Vec<&str> = chunks.iter().map(|c| c.id.as_str()).collect();
        let doc_ids: Vec<&str> = chunks.iter().map(|c| c.document_id.as_str()).collect();
        let indices: Vec<i32> = chunks.iter().map(|c| c.chunk_index as i32).collect();
        let contents: Vec<&str> = chunks.iter().map(|c| c.content.as_str()).collect();
        let metadata_json: Vec<String> = chunks
            .iter()
            .map(|_| serde_json::json!({}).to_string())
            .collect();
        let meta_refs: Vec<&str> = metadata_json.iter().map(|s| s.as_str()).collect();

        let embedding_values: Vec<f32> = chunks
            .iter()
            .flat_map(|c| {
                c.embedding
                    .as_ref()
                    .cloned()
                    .unwrap_or_else(|| vec![0.0f32; dims])
            })
            .collect();

        let embedding_field = Field::new("item", DataType::Float32, true);
        let embedding_values_arr = Arc::new(Float32Array::from(embedding_values));
        let embedding_array = FixedSizeListArray::try_new(
            Arc::new(embedding_field),
            dims as i32,
            embedding_values_arr,
            None, /* nulls */
        )
        .map_err(|e| AgentJaxError::embedding(format!("Failed to create embedding array: {e}")))?;

        let batch = RecordBatch::try_new(
            self.make_schema(dims),
            vec![
                Arc::new(StringArray::from(ids)) as ArrayRef,
                Arc::new(StringArray::from(doc_ids)) as ArrayRef,
                Arc::new(Int32Array::from(indices)) as ArrayRef,
                Arc::new(StringArray::from(contents)) as ArrayRef,
                Arc::new(StringArray::from(meta_refs)) as ArrayRef,
                Arc::new(embedding_array) as ArrayRef,
            ],
        )
        .map_err(|e| AgentJaxError::embedding(format!("Failed to create RecordBatch: {e}")))?;

        Ok(batch)
    }

    fn parse_results(
        &self,
        batches: &[RecordBatch],
        min_score: f32,
    ) -> AgentJaxResult<Vec<SearchResult>> {
        let mut results = Vec::new();

        for batch in batches {
            let ids = Self::string_column(batch, "id")?;
            let doc_ids = Self::string_column(batch, "document_id")?;
            let contents = Self::string_column(batch, "content")?;
            let metas = Self::string_column(batch, "metadata")?;

            // Try to get _distance column (LanceDB uses this for vector distance)
            let distances: Option<Vec<f32>> = batch.column_by_name("_distance").map(|c| {
                c.as_any()
                    .downcast_ref::<Float32Array>()
                    .map(|arr| arr.values().to_vec())
                    .unwrap_or_default()
            });

            for i in 0..batch.num_rows() {
                let score = distances
                    .as_ref()
                    .and_then(|d| d.get(i).copied())
                    .map(|d| 1.0 / (1.0 + d)) // Convert distance to similarity score
                    .unwrap_or(1.0);

                if score < min_score {
                    continue;
                }

                let metadata: BTreeMap<String, String> = if metas.is_null(i) {
                    BTreeMap::new()
                } else {
                    serde_json::from_str(metas.value(i)).unwrap_or_default()
                };

                results.push(SearchResult {
                    chunk_id: ids.value(i).to_string(),
                    document_id: doc_ids.value(i).to_string(),
                    content: contents.value(i).to_string(),
                    score,
                    metadata,
                });
            }
        }

        // Sort by score descending
        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_make_schema() {
        let store = VectorStore {
            db_path: "/tmp/test".to_string(),
        };
        let schema = store.make_schema(1536);
        assert_eq!(schema.fields().len(), 6);
        assert_eq!(
            schema.field_with_name("id").unwrap().data_type(),
            &DataType::Utf8
        );
        assert_eq!(
            schema.field_with_name("chunk_index").unwrap().data_type(),
            &DataType::Int32
        );
    }
}
