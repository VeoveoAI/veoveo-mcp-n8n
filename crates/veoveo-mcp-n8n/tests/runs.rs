//! Runs as durable MCP Tasks through the SDK's in-process mesh, against a migrated
//! database and the fake n8n in `support`: a run completes from its Veoveo Result
//! callback, fails and ends as cancelled from n8n's execution through the status check,
//! is cancelled by its caller, which stops the execution, and a restarted worker keeps
//! waiting on the recorded execution instead of dispatching again. n8n's side is the
//! fake's; the real n8n's run is `tests/workflows.rs`. Requires the PostgreSQL 18 server
//! programs on `PATH`.
mod support;

use std::time::Duration;

use serde_json::json;
use support::mesh::{Harness, TRIGGER_WEBHOOK, harness, status};

async fn within<T>(case: impl Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(150), case)
        .await
        .expect("the run test exceeded 150 seconds")
}

async fn post_result(
    harness: &Harness,
    callback: &str,
    execution: u64,
    output: serde_json::Value,
) -> u16 {
    let _ = harness;
    reqwest::Client::new()
        .post(callback)
        .header("content-type", "application/json")
        .body(json!({"executionId": execution.to_string(), "output": output}).to_string())
        .send()
        .await
        .unwrap()
        .status()
        .as_u16()
}

#[tokio::test]
async fn a_run_completes_fails_is_cancelled_and_survives_its_worker() {
    within(async {
        let mut harness = harness(&["n8n.workflows.run", "n8n.workflows.read"]).await;

        // The worker calls the trigger with the operation, a fresh token, the callback,
        // the bridge and the input, and the Veoveo Result's callback completes the Task.
        let task = harness.run(json!({"name": "Ada"})).await;
        let dispatch = harness.dispatched(1).await;
        let headers = harness.n8n.state().dispatches[0].0.clone();
        assert_eq!(headers["x-veoveo-dispatch"], "run");
        assert_eq!(dispatch["input"], json!({"name": "Ada"}));
        assert_eq!(dispatch["runToken"].as_str().unwrap().len(), 64);
        let callback = dispatch["callbackUrl"]
            .as_str()
            .expect("a callback URL")
            .to_owned();
        assert!(
            callback.starts_with(&format!("{}n8n/operations/", harness.loopback)),
            "{callback}"
        );
        assert_eq!(
            dispatch["bridgeUrl"],
            format!("{}n8n/bridge/calls", harness.loopback)
        );
        let execution = harness.execution_of(&dispatch);
        // A wrong token or operation is not the run's.
        let forged = format!("{}x", callback.trim_end_matches(|c: char| c != '/'));
        assert_eq!(
            post_result(&harness, &forged, execution, json!([])).await,
            404
        );
        assert_eq!(
            post_result(
                &harness,
                &callback,
                execution,
                json!([{"greeting": "Hello, Ada"}])
            )
            .await,
            204
        );
        harness
            .until(&task, |view| status(view) == "completed")
            .await;
        let stored = harness
            .worker
            .as_ref()
            .unwrap()
            .runtime
            .get(veoveo_types::TaskId::parse(&task).unwrap())
            .await
            .unwrap()
            .unwrap();
        let result = stored.result.expect("the completed Task's result");
        assert_eq!(
            result["structuredContent"]["output"],
            json!([{"greeting": "Hello, Ada"}]),
            "{result}"
        );
        assert_eq!(
            result["structuredContent"]["executionUri"],
            format!("n8n://execution/{execution}")
        );

        // A failed execution fails the Task, from the status check.
        let task = harness.run(json!({})).await;
        let dispatch = harness.dispatched(2).await;
        harness
            .n8n
            .settle(harness.execution_of(&dispatch), "error", vec![]);
        let view = harness.until(&task, |view| status(view) == "failed").await;
        assert!(view.to_string().contains("the step failed"), "{view}");

        // An execution someone stopped in n8n fails the Task: only its caller cancels it.
        let task = harness.run(json!({})).await;
        let dispatch = harness.dispatched(3).await;
        harness
            .n8n
            .settle(harness.execution_of(&dispatch), "canceled", vec![]);
        let view = harness.until(&task, |view| status(view) == "failed").await;
        assert!(view.to_string().contains("was stopped in n8n"), "{view}");

        // The caller's cancellation stops the execution.
        let task = harness.run(json!({})).await;
        let dispatch = harness.dispatched(4).await;
        let execution = harness.execution_of(&dispatch);
        harness.until(&task, |view| status(view) == "working").await;
        let cancelled = harness.mesh.cancel_task("n8n", &task).await;
        assert!(cancelled["error"].is_null(), "{cancelled}");
        harness
            .until(&task, |view| status(view) == "cancelled")
            .await;
        assert!(
            harness.stops().contains(&execution),
            "the execution was not stopped"
        );

        // A restarted worker keeps waiting on the recorded execution.
        let task = harness.run(json!({"name": "Grace"})).await;
        let dispatch = harness.dispatched(5).await;
        let execution = harness.execution_of(&dispatch);
        harness.worker.take().unwrap().stop().await;
        harness.worker = Some(harness.start_worker(None).await);
        harness.n8n.settle(
            execution,
            "success",
            vec![json!({"greeting": "Hello, Grace"})],
        );
        let view = harness
            .until(&task, |view| status(view) == "completed")
            .await;
        assert_eq!(
            harness.dispatch_count(),
            5,
            "the restarted worker dispatched again: {view}"
        );
        let _ = TRIGGER_WEBHOOK;
    })
    .await;
}
