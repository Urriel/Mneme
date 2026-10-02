use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    ProtocolVersion, ReadResourceRequestParams, ReadResourceResponse, ReadResourceResult,
    ResourceContents, ResourceTemplate, ServerCapabilities,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, Json, RoleServer, ServerHandler, tool, tool_handler, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{Error, Mneme};

#[derive(Clone)]
pub(crate) struct Mcp {
    mneme: Mneme,
}

impl Mcp {
    pub(crate) fn new(mneme: Mneme) -> Self {
        Self { mneme }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
struct IngestArgs {
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    title: String,
    #[serde(default)]
    author: String,
}

#[derive(Debug, Serialize, JsonSchema)]
struct IdOut {
    doc_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct SearchArgs {
    query: String,
    #[serde(default)]
    k: Option<u32>,
    #[serde(default)]
    as_of: Option<u64>,
    #[serde(default)]
    include_superseded: Option<bool>,
}

#[derive(Debug, Serialize, JsonSchema)]
struct SearchOut {
    hits: Vec<crate::Hit>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct NoteSearchArgs {
    query: String,
    #[serde(default)]
    k: Option<u32>,
}

#[derive(Debug, Serialize, JsonSchema)]
struct NoteSearchOut {
    hits: Vec<crate::NoteHit>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct GetArgs {
    id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct RecentArgs {
    #[serde(default)]
    since: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
struct RecentOut {
    rows: Vec<crate::Recent>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct NoteArgs {
    text: String,
    source_doc_id: String,
    #[serde(default)]
    links: Vec<String>,
    #[serde(default)]
    supersedes: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
struct NoteOut {
    id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct LinkArgs {
    from_id: String,
    to_id: String,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
struct CursorArgs {
    dreamed_until: String,
}

#[tool_router]
impl Mcp {
    #[tool(
        name = "ingest",
        description = "Store a document from a file path or from text. Returns the document id. The same bytes return the same id."
    )]
    async fn ingest(
        &self,
        Parameters(args): Parameters<IngestArgs>,
    ) -> Result<Json<IdOut>, ErrorData> {
        let id = match (args.path, args.text) {
            (Some(path), None) => self
                .mneme
                .ingest_path(std::path::Path::new(&path), &args.title, &args.author)
                .await
                .map_err(error_data)?,
            (None, Some(text)) => self
                .mneme
                .ingest_text(&text, &args.title, &args.author)
                .await
                .map_err(error_data)?,
            _ => {
                return Err(ErrorData::invalid_params(
                    "pass path or text, not both",
                    None,
                ));
            }
        };
        Ok(Json(IdOut { doc_id: id }))
    }

    #[tool(
        name = "search_doc",
        description = "Search documents by meaning and words. Returns doc_id, title, path, score, and a short excerpt."
    )]
    async fn search_doc(
        &self,
        Parameters(args): Parameters<SearchArgs>,
    ) -> Result<Json<SearchOut>, ErrorData> {
        let k = args.k.unwrap_or(5) as usize;
        let hits = self
            .mneme
            .search(
                &args.query,
                k,
                args.as_of,
                args.include_superseded.unwrap_or(false),
            )
            .await
            .map_err(error_data)?;
        Ok(Json(SearchOut { hits }))
    }

    #[tool(
        name = "search_note",
        description = "Search notes by meaning and words. Returns id, source_doc_id, score, and a short excerpt. A replaced note stays out of the hits."
    )]
    async fn search_note(
        &self,
        Parameters(args): Parameters<NoteSearchArgs>,
    ) -> Result<Json<NoteSearchOut>, ErrorData> {
        let k = args.k.unwrap_or(5) as usize;
        let hits = self
            .mneme
            .search_notes(&args.query, k)
            .await
            .map_err(error_data)?;
        Ok(Json(NoteSearchOut { hits }))
    }

    #[tool(
        name = "get",
        description = "Read one document or one note by id. Returns the full row."
    )]
    async fn get(
        &self,
        Parameters(args): Parameters<GetArgs>,
    ) -> Result<Json<crate::Document>, ErrorData> {
        let doc = self.mneme.read(&args.id).await.map_err(error_data)?;
        Ok(Json(doc))
    }

    #[tool(
        name = "list_recent",
        description = "List documents and notes ingested after since. since defaults to the dream cursor."
    )]
    async fn list_recent(
        &self,
        Parameters(args): Parameters<RecentArgs>,
    ) -> Result<Json<RecentOut>, ErrorData> {
        let rows = self
            .mneme
            .list_recent(args.since.as_deref())
            .await
            .map_err(error_data)?;
        Ok(Json(RecentOut { rows }))
    }

    #[tool(
        name = "add_note",
        description = "Store one note and embed it. Does not change the source document."
    )]
    async fn add_note(
        &self,
        Parameters(args): Parameters<NoteArgs>,
    ) -> Result<Json<NoteOut>, ErrorData> {
        let id = self
            .mneme
            .add_note(
                &args.text,
                &args.source_doc_id,
                &args.links,
                args.supersedes.as_deref(),
            )
            .await
            .map_err(error_data)?;
        Ok(Json(NoteOut { id }))
    }

    #[tool(name = "link", description = "Append to_id to the links of a note.")]
    async fn link(
        &self,
        Parameters(args): Parameters<LinkArgs>,
    ) -> Result<Json<NoteOut>, ErrorData> {
        self.mneme
            .link(&args.from_id, &args.to_id)
            .await
            .map_err(error_data)?;
        Ok(Json(NoteOut { id: args.from_id }))
    }

    #[tool(
        name = "set_cursor",
        description = "Advance the dream cursor to dreamed_until. The dream skill is the caller."
    )]
    async fn set_cursor(
        &self,
        Parameters(args): Parameters<CursorArgs>,
    ) -> Result<Json<CursorArgs>, ErrorData> {
        self.mneme
            .set_cursor(&args.dreamed_until)
            .await
            .map_err(error_data)?;
        Ok(Json(args))
    }
}

#[tool_handler]
impl ServerHandler for Mcp {
    fn get_info(&self) -> rmcp::model::ServerConfig {
        rmcp::model::ServerConfig::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .build(),
        )
        .with_protocol_version(ProtocolVersion::V_2025_06_18)
        .with_instructions(
            "search_doc and search_note return an excerpt. Call get or read mneme://doc/{id} for the full text.",
        )
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        let id = request
            .uri
            .strip_prefix("mneme://doc/")
            .ok_or_else(|| ErrorData::invalid_params("uri must be mneme://doc/{id}", None))?;
        let doc = self.mneme.read(id).await.map_err(error_data)?;
        let body = serde_json::to_string(&doc)
            .map_err(|err| ErrorData::internal_error(err.to_string(), None))?;
        Ok(ReadResourceResult::new(vec![
            ResourceContents::text(body, request.uri).with_mime_type("application/json"),
        ])
        .into())
    }

    async fn list_resource_templates(
        &self,
        _request: Option<rmcp::model::PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<rmcp::model::ListResourceTemplatesResult, ErrorData> {
        Ok(rmcp::model::ListResourceTemplatesResult::with_all_items(
            vec![
                ResourceTemplate::new("mneme://doc/{id}", "doc").with_mime_type("application/json"),
            ],
        ))
    }
}

fn error_data(err: Error) -> ErrorData {
    let invalid = matches!(
        err,
        Error::EmptyText
            | Error::BadTime
            | Error::NotFound
            | Error::NotANote
            | Error::CursorMovedBack
            | Error::BadId
    );
    let message = err.to_string();
    if invalid {
        ErrorData::invalid_params(message, None)
    } else {
        ErrorData::internal_error(message, None)
    }
}
