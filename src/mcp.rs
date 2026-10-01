use rmcp::handler::server::wrapper::Parameters;
use rmcp::{ErrorData, Json, tool, tool_router};
use serde::{Deserialize, Serialize};

use crate::{Brain, BrainError, Hit, Limit, Metadata, Query, Text};

#[derive(Clone)]
pub(crate) struct Mcp {
    brain: Brain,
}

impl Mcp {
    pub(crate) fn new(brain: Brain) -> Self {
        Self { brain }
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct AddArgs {
    text: String,
    #[serde(default)]
    metadata: Option<serde_json::Value>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
struct AddOut {
    id: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct SearchArgs {
    query: String,
    #[serde(default)]
    limit: Option<u32>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
struct HitOut {
    id: String,
    text: String,
    score: f32,
    metadata: serde_json::Value,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
struct SearchOut {
    hits: Vec<HitOut>,
}

impl From<Hit> for HitOut {
    fn from(hit: Hit) -> Self {
        Self {
            id: hit.id.to_string(),
            text: hit.text,
            score: hit.score,
            metadata: hit.metadata,
        }
    }
}

#[tool_router(server_handler)]
impl Mcp {
    #[tool(
        name = "memory_add",
        description = "Store one note. text is required. metadata is an optional JSON object."
    )]
    async fn memory_add(
        &self,
        Parameters(args): Parameters<AddArgs>,
    ) -> Result<Json<AddOut>, ErrorData> {
        let text = Text::new(args.text).map_err(error_data)?;
        let metadata = Metadata::from_json(args.metadata).map_err(error_data)?;
        let id = self.brain.add(text, metadata).await.map_err(error_data)?;
        Ok(Json(AddOut { id: id.to_string() }))
    }

    #[tool(
        name = "memory_search",
        description = "Search notes by meaning. query is required. limit defaults to 5 and must be from 1 to 100."
    )]
    async fn memory_search(
        &self,
        Parameters(args): Parameters<SearchArgs>,
    ) -> Result<Json<SearchOut>, ErrorData> {
        let query = Query::new(args.query).map_err(error_data)?;
        let limit = Limit::new(args.limit).map_err(error_data)?;
        let hits = self.brain.search(query, limit).await.map_err(error_data)?;
        Ok(Json(SearchOut {
            hits: hits.into_iter().map(HitOut::from).collect(),
        }))
    }
}

fn error_data(err: BrainError) -> ErrorData {
    let invalid = matches!(
        &err,
        BrainError::EmptyText
            | BrainError::EmptyQuery
            | BrainError::BadLimit
            | BrainError::MetadataNotObject
            | BrainError::MetadataTooLarge
    );
    let message = err.to_string();
    if invalid {
        ErrorData::invalid_params(message, None)
    } else {
        ErrorData::internal_error(message, None)
    }
}
