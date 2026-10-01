mod brain;
mod mcp;

use std::path::Path;

use rmcp::ServiceExt;
use rmcp::transport::io::stdio;

pub use brain::{Brain, BrainError, Hit, Limit, MemoryId, Metadata, Query, Text};

pub async fn serve(data_dir: &Path) -> Result<(), BrainError> {
    let brain = Brain::open(data_dir).await?;
    let running = mcp::Mcp::new(brain)
        .serve(stdio())
        .await
        .map_err(|err| BrainError::Store(err.to_string()))?;
    running
        .waiting()
        .await
        .map_err(|err| BrainError::Store(err.to_string()))?;
    Ok(())
}
