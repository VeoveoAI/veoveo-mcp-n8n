//! Embeds the skills under `skills/`, which the server serves at `skill://n8n/…`.
fn main() {
    veoveo_skills::build::embed_skills("skills").expect("n8n's skills");
}
