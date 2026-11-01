//! n8n in a person's own Computer; the program is `veoveo_mcp_n8n::computer`.
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    veoveo_mcp_n8n::computer::run().await
}
