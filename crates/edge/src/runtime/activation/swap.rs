use std::sync::Arc;

use impulse_errors::ProxyError;

use super::{service::StagedRuntimeReloadPlan, *};
use crate::runtime::{
    bundle::{
        ActiveRuntimeGeneration, RuntimeBundle, RuntimeBundleHandle, RuntimeGenerationRecordStatus,
    },
    generation::CarriedProcessSharedServices,
    listener::QUICListener,
};

pub(super) fn commit_staged_runtime_reload(
    handle: &RuntimeBundleHandle,
    plan: StagedRuntimeReloadPlan,
) -> Result<(GenerationId, ReloadDiff), (GenerationId, ReloadDiff, String)> {
    let candidate_generation = plan.plan.candidate_generation;
    let diff = plan.plan.diff.clone();
    let next_runtime = plan.next_runtime.ok_or_else(|| {
        (
            candidate_generation,
            diff.clone(),
            "staged reload plan missing next runtime".to_string(),
        )
    })?;

    commit_runtime_bundle_swap(
        handle,
        next_runtime,
        RuntimeGenerationRecordStatus::Previous,
        plan.plan.current_generation,
    )
    .map(|generation| (generation, diff.clone()))
    .map_err(|err| (candidate_generation, diff, err.to_string()))
}

pub(super) fn commit_runtime_bundle_swap(
    handle: &RuntimeBundleHandle,
    next_runtime: RuntimeBundle,
    previous_status: RuntimeGenerationRecordStatus,
    expected_generation: Option<GenerationId>,
) -> Result<GenerationId, ProxyError> {
    QUICListener::spawn_generation_background_tasks_for_runtime(
        &next_runtime.runtime_config,
        next_runtime.shared_state.as_ref(),
    );
    handle.replace_with_archive_status(next_runtime, previous_status, expected_generation)
}

pub(super) fn prepare_rollback_bundle(
    current: &ActiveRuntimeGeneration,
    target: &RuntimeBundle,
    candidate_generation: GenerationId,
) -> Result<RuntimeBundle, Box<RejectedChange>> {
    let carried = CarriedProcessSharedServices::from_active(current.shared_services());
    let next_shared_state =
        QUICListener::build_shared_state_with_carried(&target.runtime_config, Some(carried))
            .map_err(|err| {
                Box::new(RejectedChange::resource_preparation_failed(
                    "runtime rollback",
                    err.to_string(),
                ))
            })?;

    Ok(RuntimeBundle {
        generation: candidate_generation,
        startup: target.startup.clone(),
        runtime_config: target.runtime_config.clone(),
        shared_state: Arc::new(next_shared_state),
    })
}
