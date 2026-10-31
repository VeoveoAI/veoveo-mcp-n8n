//! A run's operation's job: the n8n instance and its execution, `{instance}:{execution}`.
//! n8n numbers executions from 1 in each database, so the number alone would name an
//! earlier instance's execution once n8n's data starts again over a reset schema.
use veoveo_task::ExternalJobId;

use crate::contract::ExecutionId;

/// The job of `execution` of n8n `instance`.
pub fn job(instance: uuid::Uuid, execution: &ExecutionId) -> ExternalJobId {
    ExternalJobId::parse(format!("{instance}:{execution}")).expect("an instance and an execution")
}

/// The execution a job names. A job recorded before it named its instance is the bare
/// execution number.
pub fn execution(job: &ExternalJobId) -> Option<ExecutionId> {
    let execution = job
        .as_str()
        .rsplit_once(':')
        .map_or(job.as_str(), |(_, execution)| execution);
    ExecutionId::parse(execution).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_job_names_its_instance_and_execution() {
        let first = uuid::Uuid::from_u128(1);
        let second = uuid::Uuid::from_u128(2);
        let two = ExecutionId::parse("2").unwrap();
        assert_ne!(job(first, &two), job(second, &two));
        assert_eq!(
            job(first, &two).as_str(),
            "00000000-0000-0000-0000-000000000001:2"
        );
        assert_eq!(execution(&job(first, &two)), Some(two.clone()));
        assert_eq!(execution(&ExternalJobId::parse("2").unwrap()), Some(two));
        assert_eq!(execution(&ExternalJobId::parse("x:y").unwrap()), None);
    }
}
