//! The server's SQL names only the `n8n` schema. Core state comes through
//! veoveo-hub-sdk and core's runtime API.

#[test]
fn sql_names_only_the_n8n_schema() {
    veoveo_hub_sdk::testing::assert_sql_confined(env!("CARGO_MANIFEST_DIR"), "n8n");
}
