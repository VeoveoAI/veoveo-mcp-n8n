//! The bridge through the SDK's in-process mesh: a running run's token calls a tool of the
//! bridge table for the run's caller, through the mesh's exchange of the run's Task, with
//! the permission the server declared in its `uses`; a tool outside the table, a token of
//! no run and the token of a run that ended are refused, and a caller without the
//! provider's permission gets the Hub's refusal. The fake Time server in `support` stands
//! for Time. Requires the PostgreSQL 18 server programs on `PATH`.
mod support;

use std::time::Duration;

use serde_json::{Value, json};
use support::mesh::{Harness, harness, status};

async fn bridge(harness: &Harness, body: Value) -> (u16, Value) {
    let response = reqwest::Client::new()
        .post(format!("{}n8n/bridge/calls", harness.loopback))
        .header("content-type", "application/json")
        .body(body.to_string())
        .send()
        .await
        .unwrap();
    let status = response.status().as_u16();
    (status, response.json().await.unwrap_or(Value::Null))
}

fn call(token: &str, tool: &str) -> Value {
    json!({
        "runToken": token,
        "tool": tool,
        "arguments": {"expression": {"format": "rfc3339", "value": "2026-10-31T12:00:00Z"}}
    })
}

#[tokio::test]
async fn the_bridge_calls_only_table_tools_with_a_running_runs_token() {
    tokio::time::timeout(Duration::from_secs(120), async {
        let harness = harness(&[
            "n8n.workflows.run",
            "n8n.workflows.read",
            "time.authorities.read",
        ])
        .await;
        let task = harness.run(json!({})).await;
        let dispatch = harness.dispatched(1).await;
        let token = dispatch["runToken"].as_str().unwrap().to_owned();
        harness.until(&task, |view| status(view) == "working").await;

        // A table tool acts for the run's caller, through the server and the run's Task.
        let (code, answer) = bridge(&harness, call(&token, "time__resolve_time")).await;
        assert_eq!(code, 200, "{answer}");
        assert_eq!(answer["isError"], false, "{answer}");
        assert_eq!(
            answer["result"]["expression"]["value"],
            "2026-10-31T12:00:00Z"
        );
        assert_eq!(answer["result"]["caller"], "https://idp.example.com#user-1");
        assert_eq!(answer["result"]["actingServer"], "n8n");
        assert_eq!(answer["result"]["actingTask"], true);

        // Another tool, even of the same server, is not in the table.
        let (code, answer) = bridge(&harness, call(&token, "time__create_event")).await;
        assert_eq!(code, 403, "{answer}");
        // A token of no run is refused.
        let (code, answer) = bridge(&harness, call(&"0".repeat(64), "time__resolve_time")).await;
        assert_eq!(code, 403, "{answer}");

        // Once the run ends its token calls nothing.
        let callback = dispatch["callbackUrl"].as_str().unwrap();
        let execution = harness.execution_of(&dispatch);
        let delivered = reqwest::Client::new()
            .post(callback)
            .header("content-type", "application/json")
            .body(json!({"executionId": execution.to_string(), "output": []}).to_string())
            .send()
            .await
            .unwrap();
        assert_eq!(delivered.status().as_u16(), 204);
        harness
            .until(&task, |view| status(view) == "completed")
            .await;
        let (code, answer) = bridge(&harness, call(&token, "time__resolve_time")).await;
        assert_eq!(code, 403, "{answer}");
    })
    .await
    .expect("the bridge test exceeded 120 seconds");
}

#[tokio::test]
async fn a_caller_without_the_providers_permission_is_refused_by_the_hub() {
    tokio::time::timeout(Duration::from_secs(120), async {
        let harness = harness(&["n8n.workflows.run", "n8n.workflows.read"]).await;
        let task = harness.run(json!({})).await;
        let dispatch = harness.dispatched(1).await;
        harness.until(&task, |view| status(view) == "working").await;
        let (code, answer) = bridge(
            &harness,
            call(dispatch["runToken"].as_str().unwrap(), "time__resolve_time"),
        )
        .await;
        assert_eq!(code, 200, "{answer}");
        assert_eq!(answer["isError"], true, "{answer}");
    })
    .await
    .expect("the bridge test exceeded 120 seconds");
}
