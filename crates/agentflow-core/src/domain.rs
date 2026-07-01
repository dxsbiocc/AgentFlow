use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StepStatus {
    Draft,
    WaitingForInput,
    Ready,
    Running,
    Completed,
    CompletedWithWarning,
    Failed,
    Skipped,
    Superseded,
}

impl StepStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::WaitingForInput => "waiting_for_input",
            Self::Ready => "ready",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::CompletedWithWarning => "completed_with_warning",
            Self::Failed => "failed",
            Self::Skipped => "skipped",
            Self::Superseded => "superseded",
        }
    }

    pub fn can_transition_to(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::Draft, Self::WaitingForInput)
                | (Self::Draft, Self::Ready)
                | (Self::WaitingForInput, Self::Ready)
                | (Self::Ready, Self::Running)
                | (Self::Running, Self::Completed)
                | (Self::Running, Self::CompletedWithWarning)
                | (Self::Running, Self::Failed)
                | (Self::Failed, Self::Ready)
                | (Self::Ready, Self::Skipped)
                | (Self::Completed, Self::Superseded)
                | (Self::CompletedWithWarning, Self::Superseded)
        )
    }

    pub fn parse(input: &str) -> Option<Self> {
        match input {
            "draft" => Some(Self::Draft),
            "waiting_for_input" => Some(Self::WaitingForInput),
            "ready" => Some(Self::Ready),
            "running" => Some(Self::Running),
            "completed" => Some(Self::Completed),
            "completed_with_warning" => Some(Self::CompletedWithWarning),
            "failed" => Some(Self::Failed),
            "skipped" => Some(Self::Skipped),
            "superseded" => Some(Self::Superseded),
            _ => None,
        }
    }
}

impl fmt::Display for StepStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RunAttemptStatus {
    Created,
    Running,
    Submitted,
    Succeeded,
    Failed,
    TimedOut,
    Cancelled,
    CacheHit,
}

impl RunAttemptStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Running => "running",
            Self::Submitted => "submitted",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::TimedOut => "timed_out",
            Self::Cancelled => "cancelled",
            Self::CacheHit => "cache_hit",
        }
    }

    pub fn parse(input: &str) -> Option<Self> {
        match input {
            "created" => Some(Self::Created),
            "running" => Some(Self::Running),
            "submitted" => Some(Self::Submitted),
            "succeeded" => Some(Self::Succeeded),
            "failed" => Some(Self::Failed),
            "timed_out" => Some(Self::TimedOut),
            "cancelled" => Some(Self::Cancelled),
            "cache_hit" => Some(Self::CacheHit),
            _ => None,
        }
    }
}

impl fmt::Display for RunAttemptStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for RunAttemptStatus {
    type Err = ();

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        Self::parse(input).ok_or(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ToolMaturity {
    Verified,
    Wrapped,
    Exploratory,
}

impl ToolMaturity {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Verified => "verified",
            Self::Wrapped => "wrapped",
            Self::Exploratory => "exploratory",
        }
    }

    pub fn parse(input: &str) -> Option<Self> {
        match input {
            "verified" => Some(Self::Verified),
            "wrapped" => Some(Self::Wrapped),
            "exploratory" => Some(Self::Exploratory),
            _ => None,
        }
    }
}

impl fmt::Display for ToolMaturity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ArtifactKind {
    Imported,
    Computed,
    Report,
    Log,
    Summary,
}

impl ArtifactKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Imported => "imported",
            Self::Computed => "computed",
            Self::Report => "report",
            Self::Log => "log",
            Self::Summary => "summary",
        }
    }

    pub fn parse(input: &str) -> Option<Self> {
        match input {
            "imported" => Some(Self::Imported),
            "computed" => Some(Self::Computed),
            "report" => Some(Self::Report),
            "log" => Some(Self::Log),
            "summary" => Some(Self::Summary),
            _ => None,
        }
    }
}

impl fmt::Display for ArtifactKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_status_names_match_v0_contract() {
        let names = [
            StepStatus::Draft.as_str(),
            StepStatus::WaitingForInput.as_str(),
            StepStatus::Ready.as_str(),
            StepStatus::Running.as_str(),
            StepStatus::Completed.as_str(),
            StepStatus::CompletedWithWarning.as_str(),
            StepStatus::Failed.as_str(),
            StepStatus::Skipped.as_str(),
            StepStatus::Superseded.as_str(),
        ];

        assert_eq!(
            names,
            [
                "draft",
                "waiting_for_input",
                "ready",
                "running",
                "completed",
                "completed_with_warning",
                "failed",
                "skipped",
                "superseded",
            ]
        );
    }

    #[test]
    fn step_status_allows_only_v0_transitions() {
        assert!(StepStatus::Draft.can_transition_to(StepStatus::WaitingForInput));
        assert!(StepStatus::Draft.can_transition_to(StepStatus::Ready));
        assert!(StepStatus::Ready.can_transition_to(StepStatus::Running));
        assert!(StepStatus::Running.can_transition_to(StepStatus::Completed));
        assert!(StepStatus::Running.can_transition_to(StepStatus::CompletedWithWarning));
        assert!(StepStatus::Running.can_transition_to(StepStatus::Failed));
        assert!(StepStatus::Failed.can_transition_to(StepStatus::Ready));
        assert!(StepStatus::Ready.can_transition_to(StepStatus::Skipped));
        assert!(StepStatus::Completed.can_transition_to(StepStatus::Superseded));

        assert!(!StepStatus::Draft.can_transition_to(StepStatus::Completed));
        assert!(!StepStatus::Completed.can_transition_to(StepStatus::Running));
        assert!(!StepStatus::Skipped.can_transition_to(StepStatus::Running));
    }

    #[test]
    fn run_attempt_status_names_match_v0_contract() {
        let names = [
            RunAttemptStatus::Created.as_str(),
            RunAttemptStatus::Running.as_str(),
            RunAttemptStatus::Submitted.as_str(),
            RunAttemptStatus::Succeeded.as_str(),
            RunAttemptStatus::Failed.as_str(),
            RunAttemptStatus::TimedOut.as_str(),
            RunAttemptStatus::Cancelled.as_str(),
            RunAttemptStatus::CacheHit.as_str(),
        ];

        assert_eq!(
            names,
            [
                "created",
                "running",
                "submitted",
                "succeeded",
                "failed",
                "timed_out",
                "cancelled",
                "cache_hit",
            ]
        );
    }

    #[test]
    fn run_attempt_status_round_trips_names() {
        for status in [
            RunAttemptStatus::Created,
            RunAttemptStatus::Running,
            RunAttemptStatus::Submitted,
            RunAttemptStatus::Succeeded,
            RunAttemptStatus::Failed,
            RunAttemptStatus::TimedOut,
            RunAttemptStatus::Cancelled,
            RunAttemptStatus::CacheHit,
        ] {
            assert_eq!(RunAttemptStatus::parse(status.as_str()), Some(status));
            assert_eq!(status.as_str().parse::<RunAttemptStatus>(), Ok(status));
        }

        assert_eq!(RunAttemptStatus::parse("unknown"), None);
    }

    #[test]
    fn tool_maturity_round_trips_v0_names() {
        for maturity in [
            ToolMaturity::Verified,
            ToolMaturity::Wrapped,
            ToolMaturity::Exploratory,
        ] {
            assert_eq!(
                ToolMaturity::parse(maturity.as_str()).unwrap().as_str(),
                maturity.as_str()
            );
        }

        assert_eq!(ToolMaturity::parse("unknown"), None);
    }

    #[test]
    fn artifact_kind_round_trips_v0_names() {
        for kind in [
            ArtifactKind::Imported,
            ArtifactKind::Computed,
            ArtifactKind::Report,
            ArtifactKind::Log,
            ArtifactKind::Summary,
        ] {
            assert_eq!(
                ArtifactKind::parse(kind.as_str()).unwrap().as_str(),
                kind.as_str()
            );
        }

        assert_eq!(ArtifactKind::parse("unknown"), None);
    }
}
