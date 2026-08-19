#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BackendClientRotationState {
    MissingBackend,
    Unchanged,
    Recreated,
    Rotated {
        previous_generation: u64,
        current_generation: u64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BackendClientRotation {
    state: BackendClientRotationState,
}

impl BackendClientRotation {
    pub(crate) fn missing_backend() -> Self {
        Self {
            state: BackendClientRotationState::MissingBackend,
        }
    }

    pub(crate) fn recreated() -> Self {
        Self {
            state: BackendClientRotationState::Recreated,
        }
    }

    pub(crate) fn unchanged(_generation: u64) -> Self {
        Self {
            state: BackendClientRotationState::Unchanged,
        }
    }

    pub(crate) fn rotated(previous_generation: u64, current_generation: u64) -> Self {
        Self {
            state: BackendClientRotationState::Rotated {
                previous_generation,
                current_generation,
            },
        }
    }

    pub(crate) fn changed(self) -> bool {
        matches!(
            self.state,
            BackendClientRotationState::Recreated | BackendClientRotationState::Rotated { .. }
        )
    }

    pub(crate) fn generations(self) -> Option<(u64, u64)> {
        match self.state {
            BackendClientRotationState::Rotated {
                previous_generation,
                current_generation,
            } => Some((previous_generation, current_generation)),
            _ => None,
        }
    }
}
