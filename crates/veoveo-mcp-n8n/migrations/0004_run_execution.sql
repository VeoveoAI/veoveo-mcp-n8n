-- The executions the server dispatched for a run's Task, and the Task's caller: only that
-- caller reads such an execution's data. A retry of one, which only that caller may ask
-- for, is theirs too. An execution n8n holds that no row names is a webhook's, a
-- schedule's or a retry of one of those, whose data a writer of the repository reads.
CREATE TABLE n8n.run_execution (
    tenant uuid NOT NULL,
    instance_id uuid NOT NULL,
    execution_id text NOT NULL CHECK (execution_id ~ '^(0|[1-9][0-9]{0,18})$'),
    task_id uuid NOT NULL,
    caller text NOT NULL CHECK (char_length(caller) BETWEEN 1 AND 2048),
    dispatched_at timestamptz NOT NULL,
    PRIMARY KEY (tenant, instance_id, execution_id)
);

COMMENT ON TABLE n8n.run_execution IS
    'Retention: as long as n8n keeps the execution, 14 days by its default pruning. Each start deletes older rows and those of another n8n instance.';
