use super::*;
use crate::{AudioFault, FaultOrigin};
use ravelink_core::{Bus, Input, InputKind, Send, Session};
use std::{
    cell::{Cell, RefCell},
    panic::{catch_unwind, AssertUnwindSafe},
    sync::atomic::Ordering,
};

const GENEROUS_BUDGET: Duration = Duration::from_secs(30);
const LATE_BUDGET: Duration = Duration::from_millis(100);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    Start,
    Progress,
    Inputs,
    Activate,
    Cancel,
}
const ORDER: [Stage; 4] = [
    Stage::Start,
    Stage::Progress,
    Stage::Inputs,
    Stage::Activate,
];

#[derive(Clone)]
enum Action {
    Ok,
    Error(AudioError),
    Cancel,
    ReplaceEpoch,
    Fault,
    Delay(Duration),
    Late,
    Panic,
}

fn permit() -> OutputPermit {
    let mut session = Session::new("Offline native startup", 48_000);
    let input = session
        .add_input(Input::new(
            "Tone",
            InputKind::TestTone {
                frequency_hz: 440.0,
                amplitude_db: -12.0,
            },
        ))
        .unwrap();
    let bus = session.add_bus(Bus::new("Mix")).unwrap();
    session.add_send(Send::new(input, bus)).unwrap();
    crate::tests::render(&session, bus, 2).permit()
}

struct FakeOutput<'a> {
    permit: &'a OutputPermit,
    events: RefCell<Vec<Stage>>,
    changed_stage: Option<Stage>,
    action: Action,
    active: Cell<bool>,
    cancels: Cell<usize>,
    wait_budget: Cell<Option<Duration>>,
}
impl<'a> FakeOutput<'a> {
    fn new(permit: &'a OutputPermit, stage: Option<Stage>, action: Action) -> Self {
        Self {
            permit,
            events: RefCell::new(Vec::new()),
            changed_stage: stage,
            action,
            active: Cell::new(false),
            cancels: Cell::new(0),
            wait_budget: Cell::new(None),
        }
    }
    fn visit(&self, stage: Stage) -> Result<(), AudioError> {
        self.events.borrow_mut().push(stage);
        if self.changed_stage != Some(stage) {
            return Ok(());
        }
        match &self.action {
            Action::Ok => {}
            Action::Error(error) => return Err(error.clone()),
            Action::Cancel => self.permit.cancel(),
            Action::ReplaceEpoch => {
                self.permit.shared.epoch.fetch_add(1, Ordering::AcqRel);
            }
            Action::Fault => self.permit.fail_at(
                AudioFault::DeviceUnavailable,
                FaultOrigin::OutputBuffer,
                self.permit.metadata(),
            ),
            Action::Delay(delay) => std::thread::sleep(*delay),
            // A fixed short delay models an OS method that returns too late.
            // The coordinator cannot interrupt it, but must reject the result.
            Action::Late => std::thread::sleep(LATE_BUDGET + Duration::from_millis(1)),
            Action::Panic => panic!("deliberate offline startup unwind at {stage:?}"),
        }
        Ok(())
    }
    fn inputs(&self) -> Result<(), AudioError> {
        assert!(
            !self.active.get(),
            "inputs must start behind the closed gate"
        );
        self.visit(Stage::Inputs)
    }
    fn assert_cancelled_at(&self, stage: Option<Stage>) {
        let mut expected = if let Some(stage) = stage {
            ORDER[..=ORDER.iter().position(|item| *item == stage).unwrap()].to_vec()
        } else {
            Vec::new()
        };
        expected.push(Stage::Cancel);
        assert_eq!(*self.events.borrow(), expected);
        assert_eq!(self.cancels.get(), 1);
        assert!(!self.active.get());
        assert!(!self.permit.wanted.load(Ordering::Acquire));
        assert!(!self.permit.authorized());
    }
}
impl OutputStart for FakeOutput<'_> {
    fn start(&self) -> Result<(), AudioError> {
        assert!(!self.active.get());
        self.visit(Stage::Start)
    }
    fn wait_for_progress(&self, timeout: Duration) -> Result<(), AudioError> {
        assert!(!self.active.get());
        assert!(!timeout.is_zero());
        self.wait_budget.set(Some(timeout));
        self.visit(Stage::Progress)
    }
    fn activate(&self) -> Result<(), AudioError> {
        assert!(self.permit.authorized());
        assert!(!self.active.replace(true));
        self.visit(Stage::Activate)
    }
    fn cancel(&self) {
        // The shared gate must close before invoking even this local hook.
        assert!(!self.permit.wanted.load(Ordering::Acquire));
        self.events.borrow_mut().push(Stage::Cancel);
        self.cancels.set(self.cancels.get() + 1);
        self.active.set(false);
    }
}

#[test]
fn success_orders_native_start_progress_inputs_and_activation() {
    let permit = permit();
    let output = FakeOutput::new(&permit, None, Action::Ok);
    assert_eq!(
        start_output(&output, &permit, GENEROUS_BUDGET, || output.inputs()),
        Ok(())
    );
    assert_eq!(*output.events.borrow(), ORDER);
    assert!(output.active.get());
    assert!(permit.authorized());
    assert_eq!(output.cancels.get(), 0);
    assert!(output.wait_budget.get().unwrap() <= GENEROUS_BUDGET);
}

#[test]
fn native_progress_receives_only_the_budget_remaining_after_start() {
    let permit = permit();
    let spent = Duration::from_millis(2);
    let output = FakeOutput::new(&permit, Some(Stage::Start), Action::Delay(spent));
    assert_eq!(
        start_output(&output, &permit, GENEROUS_BUDGET, || output.inputs()),
        Ok(())
    );
    assert!(output.wait_budget.get().unwrap() <= GENEROUS_BUDGET - spent);
    assert!(permit.authorized());
}

#[test]
fn failed_native_start_progress_inputs_or_activation_preserves_error_and_cancels() {
    for stage in ORDER {
        let permit = permit();
        let error = if stage == Stage::Progress {
            AudioError::Timeout {
                operation: "native callback progress",
            }
        } else {
            AudioError::Backend {
                operation: "fake native operation",
                device: Some("explicit device".into()),
                detail: format!("failure at {stage:?}"),
            }
        };
        let output = FakeOutput::new(&permit, Some(stage), Action::Error(error.clone()));
        assert_eq!(
            start_output(&output, &permit, GENEROUS_BUDGET, || output.inputs()),
            Err(error)
        );
        output.assert_cancelled_at(Some(stage));
    }
}

#[test]
fn initially_cancelled_stale_or_faulted_permit_never_starts_native_io() {
    for action in [Action::Cancel, Action::ReplaceEpoch, Action::Fault] {
        let permit = permit();
        let output = FakeOutput::new(&permit, Some(Stage::Start), action);
        output.visit(Stage::Start).unwrap();
        output.events.borrow_mut().clear();
        assert_eq!(
            start_output(&output, &permit, GENEROUS_BUDGET, || output.inputs()),
            Err(AudioError::Cancelled)
        );
        output.assert_cancelled_at(None);
    }
}

#[test]
fn invalidation_after_each_stage_prevents_further_startup_and_revokes_wanted() {
    for stage in ORDER {
        for action in [Action::Cancel, Action::ReplaceEpoch, Action::Fault] {
            let permit = permit();
            let output = FakeOutput::new(&permit, Some(stage), action);
            assert_eq!(
                start_output(&output, &permit, GENEROUS_BUDGET, || output.inputs()),
                Err(AudioError::Cancelled)
            );
            output.assert_cancelled_at(Some(stage));
        }
    }
}

#[test]
fn zero_budget_does_not_start_or_activate() {
    let permit = permit();
    let output = FakeOutput::new(&permit, None, Action::Ok);
    assert_eq!(
        start_output(&output, &permit, Duration::ZERO, || output.inputs()),
        Err(AudioError::Timeout {
            operation: "output startup",
        })
    );
    output.assert_cancelled_at(None);
}

#[test]
fn late_success_at_every_stage_is_rejected_including_after_activation() {
    for stage in ORDER {
        let permit = permit();
        let output = FakeOutput::new(&permit, Some(stage), Action::Late);
        assert_eq!(
            start_output(&output, &permit, LATE_BUDGET, || output.inputs()),
            Err(AudioError::Timeout {
                operation: "output startup",
            })
        );
        output.assert_cancelled_at(Some(stage));
    }
}

#[test]
fn unwinding_at_every_stage_cancels_the_partially_started_output() {
    for stage in ORDER {
        let permit = permit();
        let output = FakeOutput::new(&permit, Some(stage), Action::Panic);
        let result = catch_unwind(AssertUnwindSafe(|| {
            start_output(&output, &permit, GENEROUS_BUDGET, || output.inputs())
        }));
        assert!(result.is_err());
        output.assert_cancelled_at(Some(stage));
    }
}

#[test]
fn very_large_budget_does_not_overflow_a_monotonic_deadline() {
    let permit = permit();
    let output = FakeOutput::new(&permit, None, Action::Ok);
    assert_eq!(
        start_output(&output, &permit, Duration::MAX, || output.inputs()),
        Ok(())
    );
    assert!(permit.authorized());
    assert_eq!(output.cancels.get(), 0);
}
