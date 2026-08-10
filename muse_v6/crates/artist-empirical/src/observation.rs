use artist_formal::InterpretedGraphRef;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Stable observation identity supplied by Mnestic or another event authority.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ObservationId(pub String);

/// Explicit identity of a clock, timescale, or event-order authority.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ClockId(pub String);

/// Signed ticks on an explicitly identified clock.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Timestamp {
    /// Clock/timescale identity.
    pub clock: ClockId,
    /// Signed tick count in that clock's declared unit.
    pub ticks: i128,
}

/// Half-open valid-time interval `[start, end)`. Open bounds are permitted.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeInterval {
    /// Inclusive lower bound.
    pub start: Option<Timestamp>,
    /// Exclusive upper bound.
    pub end: Option<Timestamp>,
}

impl TimeInterval {
    /// Checks that bounded endpoints use one clock and are correctly ordered.
    #[must_use]
    pub fn well_formed(&self) -> bool {
        match (&self.start, &self.end) {
            (Some(start), Some(end)) => start.clock == end.clock && start.ticks <= end.ticks,
            _ => true,
        }
    }
}

/// Formal description of the source that emitted or supplied an observation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Source {
    /// Stable source identity.
    pub id: String,
    /// Graph object describing authority, instrument, agent, file, user, etc.
    pub description: InterpretedGraphRef,
}

/// Formal description of how an observation was acquired.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Acquisition {
    /// Acquisition method, protocol, parser, sensor configuration, or tool call.
    pub method: InterpretedGraphRef,
    /// Optional exact event-log address.
    pub event_address: Option<String>,
}

/// Epistemic status supplied by the harness. This is recorded data and never
/// silently converted into kernel truth.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum EpistemicStatus {
    /// The harness was certain of the time-indexed report when it was issued.
    CertainAtObservation,
    /// The report is explicitly revisable; confidence is in millionths.
    Revisable { confidence_ppm: u32 },
}

/// A recorded immutable observation event. `content` is what was reported; this
/// record only establishes that the report occurred with the stated provenance.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Observation {
    /// Stable event identity.
    pub id: ObservationId,
    /// Reported formal content.
    pub content: InterpretedGraphRef,
    /// Source that emitted the report.
    pub source: Source,
    /// Acquisition process.
    pub acquisition: Acquisition,
    /// Time in the represented world to which the content applies.
    pub valid_time: TimeInterval,
    /// Time the system recorded this event.
    pub recorded_at: Timestamp,
    /// Reported certainty/revisability.
    pub epistemic_status: EpistemicStatus,
    /// Upstream observation events copied, transformed, or summarized here.
    pub derived_from: Vec<ObservationId>,
    /// Earlier immutable records explicitly superseded by this record.
    pub supersedes: Vec<ObservationId>,
}

/// Structural rejection of an observation event.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum ObservationError {
    /// Valid-time interval has mismatched clocks or reversed bounds.
    #[error("observation has malformed valid time")]
    MalformedValidTime,
    /// Revisable confidence exceeds one million parts per million.
    #[error("revisable confidence exceeds one")]
    InvalidConfidence,
    /// Event attempts to supersede itself.
    #[error("observation supersedes itself")]
    SelfSupersession,
}

impl Observation {
    /// Validates only theory-neutral structure. Current applicability and Bayesian
    /// treatment are responsibilities of Mnestic/the empirical layer.
    pub fn validate(&self) -> Result<(), ObservationError> {
        if !self.valid_time.well_formed() {
            return Err(ObservationError::MalformedValidTime);
        }
        if matches!(
            self.epistemic_status,
            EpistemicStatus::Revisable { confidence_ppm } if confidence_ppm > 1_000_000
        ) {
            return Err(ObservationError::InvalidConfidence);
        }
        if self.supersedes.contains(&self.id) {
            return Err(ObservationError::SelfSupersession);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use artist_formal::{GraphHash, InterpretedGraphHash, ObjectId};

    fn graph_ref(id: &str) -> InterpretedGraphRef {
        InterpretedGraphRef {
            submission: InterpretedGraphHash(format!("submission:{id}")),
            graph: GraphHash(format!("graph:{id}")),
            root: ObjectId::new("root"),
        }
    }

    fn observation() -> Observation {
        let clock = ClockId("utc-ns".to_owned());
        Observation {
            id: ObservationId("o1".to_owned()),
            content: graph_ref("content"),
            source: Source {
                id: "filesystem".to_owned(),
                description: graph_ref("source"),
            },
            acquisition: Acquisition {
                method: graph_ref("tool-call"),
                event_address: Some("event:1".to_owned()),
            },
            valid_time: TimeInterval {
                start: Some(Timestamp {
                    clock: clock.clone(),
                    ticks: 10,
                }),
                end: Some(Timestamp {
                    clock: clock.clone(),
                    ticks: 11,
                }),
            },
            recorded_at: Timestamp { clock, ticks: 12 },
            epistemic_status: EpistemicStatus::CertainAtObservation,
            derived_from: vec![],
            supersedes: vec![],
        }
    }

    #[test]
    fn certain_and_revisable_observations_are_structurally_valid() {
        let certain = observation();
        assert_eq!(certain.validate(), Ok(()));
        let mut revisable = certain;
        revisable.id = ObservationId("o2".to_owned());
        revisable.epistemic_status = EpistemicStatus::Revisable {
            confidence_ppm: 900_000,
        };
        revisable.supersedes = vec![ObservationId("o1".to_owned())];
        assert_eq!(revisable.validate(), Ok(()));
    }

    #[test]
    fn malformed_observations_are_rejected() {
        let mut invalid = observation();
        invalid.epistemic_status = EpistemicStatus::Revisable {
            confidence_ppm: 1_000_001,
        };
        assert_eq!(invalid.validate(), Err(ObservationError::InvalidConfidence));

        let mut invalid = observation();
        invalid.supersedes = vec![invalid.id.clone()];
        assert_eq!(invalid.validate(), Err(ObservationError::SelfSupersession));

        let mut valid = observation();
        valid.recorded_at.clock = ClockId("monotonic".to_owned());
        assert_eq!(valid.validate(), Ok(()));

        let mut invalid = observation();
        invalid
            .valid_time
            .end
            .as_mut()
            .expect("bounded interval")
            .clock = ClockId("monotonic".to_owned());
        assert_eq!(
            invalid.validate(),
            Err(ObservationError::MalformedValidTime)
        );

        let mut invalid = observation();
        invalid
            .valid_time
            .end
            .as_mut()
            .expect("bounded interval")
            .ticks = 9;
        assert_eq!(
            invalid.validate(),
            Err(ObservationError::MalformedValidTime)
        );
    }

    #[test]
    fn clock_conversion_encloses_exact_fractional_and_uncertain_results() {
        let source = ClockId("source".to_owned());
        let target = ClockId("target".to_owned());
        let exact = ClockRelation {
            source: source.clone(),
            target: target.clone(),
            offset_ticks: 3,
            scale_numerator: 2,
            scale_denominator: 1,
            uncertainty_ticks: 0,
        };
        assert_eq!(
            exact.convert(&Timestamp {
                clock: source.clone(),
                ticks: 4,
            }),
            Ok(TimeInterval {
                start: Some(Timestamp {
                    clock: target.clone(),
                    ticks: 11,
                }),
                end: Some(Timestamp {
                    clock: target.clone(),
                    ticks: 12,
                }),
            })
        );

        let fractional = ClockRelation {
            source: source.clone(),
            target: target.clone(),
            offset_ticks: 0,
            scale_numerator: 1,
            scale_denominator: 2,
            uncertainty_ticks: 0,
        };
        assert_eq!(
            fractional.convert(&Timestamp {
                clock: source.clone(),
                ticks: -1,
            }),
            Ok(TimeInterval {
                start: Some(Timestamp {
                    clock: target.clone(),
                    ticks: -1,
                }),
                end: Some(Timestamp {
                    clock: target.clone(),
                    ticks: 0,
                }),
            })
        );

        let uncertain = ClockRelation {
            source: source.clone(),
            target: target.clone(),
            offset_ticks: 0,
            scale_numerator: 1,
            scale_denominator: 1,
            uncertainty_ticks: 2,
        };
        assert_eq!(
            uncertain.convert(&Timestamp {
                clock: source,
                ticks: 10,
            }),
            Ok(TimeInterval {
                start: Some(Timestamp {
                    clock: target.clone(),
                    ticks: 8,
                }),
                end: Some(Timestamp {
                    clock: target,
                    ticks: 13,
                }),
            })
        );
    }

    #[test]
    fn clock_conversion_rejects_mismatch_invalid_scale_and_overflow() {
        let relation = ClockRelation {
            source: ClockId("source".to_owned()),
            target: ClockId("target".to_owned()),
            offset_ticks: 0,
            scale_numerator: 1,
            scale_denominator: 1,
            uncertainty_ticks: 0,
        };
        assert_eq!(
            relation.convert(&Timestamp {
                clock: ClockId("other".to_owned()),
                ticks: 0,
            }),
            Err(ClockRelationError::SourceClockMismatch)
        );

        let mut invalid = relation.clone();
        invalid.scale_denominator = 0;
        assert_eq!(invalid.validate(), Err(ClockRelationError::InvalidScale));

        let mut overflowing = relation;
        overflowing.scale_numerator = 2;
        assert_eq!(
            overflowing.convert(&Timestamp {
                clock: ClockId("source".to_owned()),
                ticks: i128::MAX,
            }),
            Err(ClockRelationError::Overflow)
        );
    }

    #[test]
    fn observation_ledger_requires_unique_prior_references() {
        let first = observation();
        let mut second = observation();
        second.id = ObservationId("o2".to_owned());
        second.derived_from = vec![first.id.clone()];
        assert_eq!(
            validate_observation_ledger(&[first.clone(), second]),
            Ok(())
        );

        assert_eq!(
            validate_observation_ledger(&[first.clone(), first.clone()]),
            Err(ObservationLedgerError::Duplicate(first.id.clone()))
        );

        let mut invalid = observation();
        invalid.id = ObservationId("o3".to_owned());
        invalid.supersedes = vec![ObservationId("future".to_owned())];
        assert_eq!(
            validate_observation_ledger(&[first, invalid.clone()]),
            Err(ObservationLedgerError::InvalidPriorReference {
                observation: invalid.id,
                target: ObservationId("future".to_owned()),
            })
        );
    }
}

/// Declared relation between two clocks, including synchronization uncertainty.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClockRelation {
    /// Source clock.
    pub source: ClockId,
    /// Target clock.
    pub target: ClockId,
    /// Target ticks corresponding to source tick zero.
    pub offset_ticks: i128,
    /// Rational scale numerator.
    pub scale_numerator: i128,
    /// Positive rational scale denominator.
    pub scale_denominator: i128,
    /// Maximum absolute conversion uncertainty in target ticks.
    pub uncertainty_ticks: u128,
}

/// Clock-relation validation failure.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum ClockRelationError {
    /// Scale numerator and denominator must both be positive.
    #[error("clock relation scale must be positive")]
    InvalidScale,
    /// Timestamp uses a different source clock.
    #[error("timestamp clock does not match the relation source clock")]
    SourceClockMismatch,
    /// Conversion arithmetic overflowed.
    #[error("clock relation conversion overflowed")]
    Overflow,
}

impl ClockRelation {
    /// Validates the rational clock mapping.
    pub fn validate(&self) -> Result<(), ClockRelationError> {
        if self.scale_numerator > 0 && self.scale_denominator > 0 {
            Ok(())
        } else {
            Err(ClockRelationError::InvalidScale)
        }
    }

    /// Converts one timestamp into an enclosing half-open interval on the target clock.
    pub fn convert(&self, timestamp: &Timestamp) -> Result<TimeInterval, ClockRelationError> {
        self.validate()?;
        if timestamp.clock != self.source {
            return Err(ClockRelationError::SourceClockMismatch);
        }
        let scaled_numerator = timestamp
            .ticks
            .checked_mul(self.scale_numerator)
            .ok_or(ClockRelationError::Overflow)?;
        let offset_numerator = self
            .offset_ticks
            .checked_mul(self.scale_denominator)
            .ok_or(ClockRelationError::Overflow)?;
        let center_numerator = scaled_numerator
            .checked_add(offset_numerator)
            .ok_or(ClockRelationError::Overflow)?;
        let uncertainty =
            i128::try_from(self.uncertainty_ticks).map_err(|_| ClockRelationError::Overflow)?;
        let uncertainty_numerator = uncertainty
            .checked_mul(self.scale_denominator)
            .ok_or(ClockRelationError::Overflow)?;
        let lower_numerator = center_numerator
            .checked_sub(uncertainty_numerator)
            .ok_or(ClockRelationError::Overflow)?;
        let upper_numerator = center_numerator
            .checked_add(uncertainty_numerator)
            .ok_or(ClockRelationError::Overflow)?;
        let start = lower_numerator.div_euclid(self.scale_denominator);
        let end = upper_numerator
            .div_euclid(self.scale_denominator)
            .checked_add(1)
            .ok_or(ClockRelationError::Overflow)?;
        Ok(TimeInterval {
            start: Some(Timestamp {
                clock: self.target.clone(),
                ticks: start,
            }),
            end: Some(Timestamp {
                clock: self.target.clone(),
                ticks: end,
            }),
        })
    }
}

/// Immutable observation ledger validation failure.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum ObservationLedgerError {
    /// Individual observation is malformed.
    #[error("invalid observation {id:?}: {source}")]
    InvalidObservation {
        id: ObservationId,
        source: ObservationError,
    },
    /// Event identity is duplicated.
    #[error("duplicate observation {0:?}")]
    Duplicate(ObservationId),
    /// Provenance or supersession references an absent/non-prior event.
    #[error("observation {observation:?} references unknown or non-prior event {target:?}")]
    InvalidPriorReference {
        observation: ObservationId,
        target: ObservationId,
    },
}

/// Validates deterministic immutable ordering and prior-event references.
pub fn validate_observation_ledger(
    observations: &[Observation],
) -> Result<(), ObservationLedgerError> {
    let mut seen = std::collections::BTreeSet::new();
    for observation in observations {
        observation
            .validate()
            .map_err(|source| ObservationLedgerError::InvalidObservation {
                id: observation.id.clone(),
                source,
            })?;
        if !seen.insert(observation.id.clone()) {
            return Err(ObservationLedgerError::Duplicate(observation.id.clone()));
        }
        for target in observation
            .derived_from
            .iter()
            .chain(&observation.supersedes)
        {
            if !seen.contains(target) || target == &observation.id {
                return Err(ObservationLedgerError::InvalidPriorReference {
                    observation: observation.id.clone(),
                    target: target.clone(),
                });
            }
        }
    }
    Ok(())
}
