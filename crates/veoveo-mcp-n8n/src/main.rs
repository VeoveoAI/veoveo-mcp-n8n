#[tokio::main]
async fn main() -> anyhow::Result<()> {
    veoveo_mcp_n8n::server::run().await
}
