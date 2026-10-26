//! Pre-DSP aggregation of independently clocked raw captures.
//!
//! The caller reads each raw capture consumer exactly once into stable planar
//! scratch. This module groups those planes without advancing a capture clock,
//! applying controls, normalizing a sum, or processing channel DSP.
use relink_core::{StereoInput, Uuid};
use std::{error::Error, fmt, mem::size_of};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CaptureGroupPlan {
    pub(crate) source_indices: Vec<usize>,
    pub(crate) input_ids: Vec<Uuid>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CaptureGroupError {
    EmptyGroup(usize),
    EmptyAliases(usize),
    InvalidSource { group: usize, source: usize },
    DuplicateSource { group: usize, source: usize },
    DuplicateInput(Uuid),
    InvalidBlockCapacity,
    MemoryOverflow,
    BudgetExceeded { required: usize, available: usize },
    AllocationFailed,
    SourceCount { expected: usize, actual: usize },
    BlockTooLarge { requested: usize, maximum: usize },
    ShortSource { source: usize, required: usize },
    NonFiniteSum { group: usize, frame: usize },
}

impl fmt::Display for CaptureGroupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyGroup(group) => write!(f, "capture group {group} has no raw sources"),
            Self::EmptyAliases(group) => write!(f, "capture group {group} has no logical input"),
            Self::InvalidSource { group, source } => write!(
                f,
                "capture group {group} refers to unavailable raw source {source}"
            ),
            Self::DuplicateSource { group, source } => {
                write!(f, "capture group {group} repeats raw source {source}")
            }
            Self::DuplicateInput(id) => write!(
                f,
                "logical input {id} belongs to more than one capture alias"
            ),
            Self::InvalidBlockCapacity => {
                f.write_str("capture group block capacity must be positive")
            }
            Self::MemoryOverflow => f.write_str("capture group memory estimate overflow"),
            Self::BudgetExceeded {
                required,
                available,
            } => write!(
                f,
                "capture groups require {required} bytes; {available} are available"
            ),
            Self::AllocationFailed => f.write_str("capture group allocation failed"),
            Self::SourceCount { expected, actual } => write!(
                f,
                "capture groups expected {expected} raw sources, received {actual}"
            ),
            Self::BlockTooLarge { requested, maximum } => write!(
                f,
                "capture group block has {requested} frames; capacity is {maximum}"
            ),
            Self::ShortSource { source, required } => write!(
                f,
                "raw source {source} has fewer than {required} stereo frames"
            ),
            Self::NonFiniteSum { group, frame } => write!(
                f,
                "capture group {group} has an unrepresentable sum at frame {frame}"
            ),
        }
    }
}
impl Error for CaptureGroupError {}

struct CaptureGroup {
    plan: CaptureGroupPlan,
    left: Vec<f32>,
    right: Vec<f32>,
    // Accumulate before the one f32 conversion, retaining cancellation and
    // headroom between process streams. An unrepresentable final sum faults;
    // it is never clipped, normalized or silently converted to silence.
    sum: Vec<[f64; 2]>,
}

pub(crate) struct CaptureGroups {
    groups: Vec<CaptureGroup>,
    source_count: usize,
    max_frames: usize,
    rendered_frames: Option<usize>,
}

impl CaptureGroups {
    /// Owned persistent bytes, excluding caller/native raw buffers and allocator
    /// bookkeeping. Plan Vec capacities are charged because they are retained.
    pub(crate) fn required_bytes(
        plans: &[CaptureGroupPlan],
        source_count: usize,
        max_frames: usize,
    ) -> Result<usize, CaptureGroupError> {
        validate(plans, source_count)?;
        if max_frames == 0 {
            return Err(CaptureGroupError::InvalidBlockCapacity);
        }
        let mut bytes = size_of::<Self>();
        add(&mut bytes, plans.len(), size_of::<CaptureGroup>())?;
        for plan in plans {
            add(
                &mut bytes,
                plan.source_indices.capacity(),
                size_of::<usize>(),
            )?;
            add(&mut bytes, plan.input_ids.capacity(), size_of::<Uuid>())?;
            add(
                &mut bytes,
                max_frames,
                2 * size_of::<f32>() + size_of::<[f64; 2]>(),
            )?;
        }
        Ok(bytes)
    }

    pub(crate) fn new(
        plans: Vec<CaptureGroupPlan>,
        source_count: usize,
        max_frames: usize,
        max_bytes: usize,
    ) -> Result<Self, CaptureGroupError> {
        let required = Self::required_bytes(&plans, source_count, max_frames)?;
        budget(required, max_bytes)?;
        let mut groups = Vec::new();
        groups
            .try_reserve_exact(plans.len())
            .map_err(|_| CaptureGroupError::AllocationFailed)?;
        for plan in plans {
            groups.push(CaptureGroup {
                plan,
                left: zeroed(max_frames, 0.0)?,
                right: zeroed(max_frames, 0.0)?,
                sum: zeroed(max_frames, [0.0; 2])?,
            });
        }
        let result = Self {
            groups,
            source_count,
            max_frames,
            rendered_frames: None,
        };
        // A Vec allocator may grant more capacity than requested. Check the
        // retained capacities too, before returning anything to the callback.
        let mut actual = size_of::<Self>();
        add(
            &mut actual,
            result.groups.capacity(),
            size_of::<CaptureGroup>(),
        )?;
        for group in &result.groups {
            add(
                &mut actual,
                group.plan.source_indices.capacity(),
                size_of::<usize>(),
            )?;
            add(
                &mut actual,
                group.plan.input_ids.capacity(),
                size_of::<Uuid>(),
            )?;
            add(&mut actual, group.left.capacity(), size_of::<f32>())?;
            add(&mut actual, group.right.capacity(), size_of::<f32>())?;
            add(&mut actual, group.sum.capacity(), size_of::<[f64; 2]>())?;
        }
        budget(actual, max_bytes)?;
        Ok(result)
    }

    /// Sum raw source planes at unity. Clones of `sources` must yield identical
    /// stable planes throughout this call, in CapturePlan source-index order.
    /// No callback allocation, deallocation, locks, or source clock advancement.
    /// Rejection invalidates readable output, and no partially summed group is
    /// exposed. Buffer dimensions are checked before modifying group scratch.
    pub(crate) fn mix<'a, I>(&mut self, sources: I, frames: usize) -> Result<(), CaptureGroupError>
    where
        I: Iterator<Item = (&'a [f32], &'a [f32])> + Clone,
    {
        self.rendered_frames = None;
        if frames > self.max_frames {
            return Err(CaptureGroupError::BlockTooLarge {
                requested: frames,
                maximum: self.max_frames,
            });
        }
        let mut actual = 0;
        for (index, (left, right)) in sources.clone().enumerate() {
            if left.len() < frames || right.len() < frames {
                return Err(CaptureGroupError::ShortSource {
                    source: index,
                    required: frames,
                });
            }
            actual += 1;
        }
        if actual != self.source_count {
            return Err(CaptureGroupError::SourceCount {
                expected: self.source_count,
                actual,
            });
        }
        for (index, group) in self.groups.iter_mut().enumerate() {
            group.sum[..frames].fill([0.0; 2]);
            for &source in &group.plan.source_indices {
                let (left, right) = sources
                    .clone()
                    .nth(source)
                    .expect("validated stable source iterator");
                for ((sum, &left), &right) in group.sum[..frames].iter_mut().zip(left).zip(right) {
                    sum[0] += f64::from(left);
                    sum[1] += f64::from(right);
                }
            }
            for (frame, ((sum, left), right)) in group.sum[..frames]
                .iter()
                .zip(&mut group.left)
                .zip(&mut group.right)
                .enumerate()
            {
                let values = [sum[0] as f32, sum[1] as f32];
                if !values[0].is_finite() || !values[1].is_finite() {
                    return Err(CaptureGroupError::NonFiniteSum {
                        group: index,
                        frame,
                    });
                }
                *left = values[0];
                *right = values[1];
            }
        }
        self.rendered_frames = Some(frames);
        Ok(())
    }

    /// Each logical input UUID appears once and aliases the group's raw sum.
    /// Nothing is yielded before a successful mix or after a rejected attempt.
    pub(crate) fn sources(&self) -> impl Iterator<Item = StereoInput<'_>> + Clone {
        let frames = self.rendered_frames;
        self.groups.iter().flat_map(move |group| {
            group.plan.input_ids.iter().filter_map(move |&id| {
                frames.map(|frames| StereoInput {
                    id,
                    left: &group.left[..frames],
                    right: &group.right[..frames],
                })
            })
        })
    }
}

fn validate(plans: &[CaptureGroupPlan], source_count: usize) -> Result<(), CaptureGroupError> {
    for (index, group) in plans.iter().enumerate() {
        if group.source_indices.is_empty() {
            return Err(CaptureGroupError::EmptyGroup(index));
        }
        if group.input_ids.is_empty() {
            return Err(CaptureGroupError::EmptyAliases(index));
        }
        for (offset, &source) in group.source_indices.iter().enumerate() {
            if source >= source_count {
                return Err(CaptureGroupError::InvalidSource {
                    group: index,
                    source,
                });
            }
            if group.source_indices[..offset].contains(&source) {
                return Err(CaptureGroupError::DuplicateSource {
                    group: index,
                    source,
                });
            }
        }
        for (offset, &id) in group.input_ids.iter().enumerate() {
            if group.input_ids[..offset].contains(&id)
                || plans[..index]
                    .iter()
                    .any(|prior| prior.input_ids.contains(&id))
            {
                return Err(CaptureGroupError::DuplicateInput(id));
            }
        }
    }
    Ok(())
}

fn add(bytes: &mut usize, count: usize, width: usize) -> Result<(), CaptureGroupError> {
    *bytes = count
        .checked_mul(width)
        .and_then(|part| bytes.checked_add(part))
        .ok_or(CaptureGroupError::MemoryOverflow)?;
    Ok(())
}
fn budget(required: usize, available: usize) -> Result<(), CaptureGroupError> {
    if required > available {
        Err(CaptureGroupError::BudgetExceeded {
            required,
            available,
        })
    } else {
        Ok(())
    }
}
fn zeroed<T: Clone>(length: usize, value: T) -> Result<Vec<T>, CaptureGroupError> {
    let mut result = Vec::new();
    result
        .try_reserve_exact(length)
        .map_err(|_| CaptureGroupError::AllocationFailed)?;
    result.resize(length, value);
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use relink_core::{
        Bus, CompiledMixer, DspConfig, Input, InputKind, ResourceBudget, Send, Session,
        StereoOutput,
    };

    fn group(indices: &[usize], ids: &[Uuid]) -> CaptureGroupPlan {
        CaptureGroupPlan {
            source_indices: indices.to_vec(),
            input_ids: ids.to_vec(),
        }
    }

    #[test]
    fn independent_raw_sources_sum_once_before_logical_controls_and_shared_aliases() {
        let mut session = Session::new("Grouped capture", 48_000);
        let mut first = Input::new(
            "Application A",
            InputKind::Application {
                application_id: "fixture".into(),
            },
        );
        first.controls.set_gain_db(-6.0206);
        let mut second = Input::new("Application A alias", first.kind.clone());
        second.controls.set_gain_db(-12.0412);
        let ids = [first.id, second.id];
        let mut bus = Bus::new("Sum");
        bus.dsp = DspConfig::default();
        let bus_id = bus.id;
        session.add_input(first).unwrap();
        session.add_input(second).unwrap();
        session.add_bus(bus).unwrap();
        for id in ids {
            session.add_send(Send::new(id, bus_id)).unwrap();
        }
        let mut mixer = CompiledMixer::compile(
            &session,
            ResourceBudget {
                max_block_frames: 4,
                max_working_bytes: 1 << 20,
            },
        )
        .unwrap();
        let mut groups = CaptureGroups::new(vec![group(&[0, 1], &ids)], 2, 4, 1 << 20).unwrap();
        let a = ([0.125; 4], [-0.25; 4]);
        let b = ([0.25; 4], [0.125; 4]);
        let raw = [(&a.0[..], &a.1[..]), (&b.0[..], &b.1[..])];
        let mut left = [0.0; 4];
        let mut right = [0.0; 4];
        crate::tests::OPERATIONS.with(|n| n.set(0));
        crate::tests::COUNTING.with(|on| on.set(true));
        let mixed = groups.mix(raw.into_iter(), 4);
        let rendered = mixer.render_sources(
            groups.sources(),
            &mut [StereoOutput {
                id: bus_id,
                left: &mut left,
                right: &mut right,
            }],
            4,
        );
        crate::tests::COUNTING.with(|on| on.set(false));
        assert_eq!(crate::tests::OPERATIONS.with(std::cell::Cell::get), 0);
        mixed.unwrap();
        rendered.unwrap();
        let aliases: Vec<_> = groups.sources().collect();
        assert_eq!(aliases.len(), 2);
        assert_eq!(aliases[0].left.as_ptr(), aliases[1].left.as_ptr());
        assert_eq!(aliases[0].left, &[0.375; 4]);
        assert!(left.iter().all(|v| (*v - 0.28125).abs() < 1e-6));
        assert!(right.iter().all(|v| (*v + 0.09375).abs() < 1e-6));
    }

    #[test]
    fn grouping_rejects_duplicate_aliases_members_and_invalid_budgets() {
        let id = Uuid::new_v4();
        for plans in [
            vec![group(&[], &[id])],
            vec![group(&[0], &[])],
            vec![group(&[1], &[id])],
            vec![group(&[0, 0], &[id])],
            vec![group(&[0], &[id, id])],
            vec![group(&[0], &[id]), group(&[0], &[id])],
        ] {
            assert!(CaptureGroups::new(plans, 1, 8, usize::MAX).is_err());
        }
        let plans = vec![group(&[0], &[id])];
        let bytes = CaptureGroups::required_bytes(&plans, 1, 8).unwrap();
        assert!(matches!(
            CaptureGroups::new(plans.clone(), 1, 8, bytes - 1),
            Err(CaptureGroupError::BudgetExceeded { .. })
        ));
        assert!(CaptureGroups::new(plans.clone(), 1, 8, bytes).is_ok());
        assert!(matches!(
            CaptureGroups::required_bytes(&plans, 1, usize::MAX),
            Err(CaptureGroupError::MemoryOverflow)
        ));
        assert!(CaptureGroups::new(plans, 1, 0, usize::MAX).is_err());
    }

    #[test]
    fn shared_raw_planes_support_distinct_groups_without_duplicate_mix_inputs() {
        let ids = [Uuid::new_v4(), Uuid::new_v4()];
        let mut groups = CaptureGroups::new(
            vec![group(&[0], &ids[..1]), group(&[0, 1], &ids[1..])],
            2,
            4,
            1 << 20,
        )
        .unwrap();
        let a = [0.25; 4];
        let b = [0.5; 4];
        groups
            .mix([(&a[..], &a[..]), (&b[..], &b[..])].into_iter(), 4)
            .unwrap();
        let outputs: Vec<_> = groups.sources().collect();
        assert_eq!(outputs[0].id, ids[0]);
        assert_eq!(outputs[1].id, ids[1]);
        assert_eq!(outputs[0].left, &[0.25; 4]);
        assert_eq!(outputs[1].left, &[0.75; 4]);
    }

    #[test]
    fn short_invalid_or_unrepresentable_attempts_do_not_expose_stale_group_audio() {
        let mut groups =
            CaptureGroups::new(vec![group(&[0, 1], &[Uuid::new_v4()])], 2, 4, 1 << 20).unwrap();
        let a = [0.25; 4];
        let short = [0.5; 1];
        let huge = [f32::MAX; 4];
        assert_eq!(groups.sources().count(), 0);
        groups
            .mix([(&a[..], &a[..]), (&a[..], &a[..])].into_iter(), 4)
            .unwrap();
        assert!(groups
            .mix([(&a[..], &a[..]), (&short[..], &short[..])].into_iter(), 4)
            .is_err());
        assert_eq!(groups.sources().count(), 0);
        assert!(groups.mix([(&a[..], &a[..])].into_iter(), 4).is_err());
        assert!(groups
            .mix(
                [(&huge[..], &huge[..]), (&huge[..], &huge[..])].into_iter(),
                4
            )
            .is_err());
        assert_eq!(groups.sources().count(), 0);
        groups
            .mix([(&a[..], &a[..]), (&a[..], &a[..])].into_iter(), 1)
            .unwrap();
        assert_eq!(groups.sources().next().unwrap().left, &[0.5]);
        groups
            .mix([(&a[..], &a[..]), (&a[..], &a[..])].into_iter(), 0)
            .unwrap();
        assert!(groups.sources().next().unwrap().left.is_empty());
    }

    #[test]
    fn instance_membership_is_resource_bounded_without_a_fixed_count_or_normalization() {
        let members: Vec<_> = (0..129).collect();
        let mut groups = CaptureGroups::new(
            vec![group(&members, &[Uuid::new_v4()])],
            members.len(),
            2,
            1 << 20,
        )
        .unwrap();
        let plane = [0.25; 2];
        groups
            .mix((0..129).map(|_| (&plane[..], &plane[..])), 2)
            .unwrap();
        assert_eq!(groups.sources().next().unwrap().left, &[32.25; 2]);
    }
}
