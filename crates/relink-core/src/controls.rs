use crate::{Session, SessionError, Uuid};
use std::{error::Error, fmt, mem::size_of};

/// A fully resolved channel/send control. Gains include stereo balance; active
/// includes mute and the appropriate input, per-mix send, or mix solo scope.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ResolvedControl {
    pub id: Uuid,
    pub gains: [f32; 2],
    pub active: bool,
}
impl ResolvedControl {
    pub(crate) fn valid(&self) -> bool {
        self.gains
            .iter()
            .all(|gain| gain.is_finite() && (0.0..=16.0).contains(gain))
    }
}

/// Prepared off the audio thread, with stable order matching a compiled graph.
/// Hosts may retain and overwrite one preallocated instance on the callback.
#[derive(Clone, Debug, PartialEq)]
pub struct PreparedControls {
    pub inputs: Vec<ResolvedControl>,
    pub sends: Vec<ResolvedControl>,
    pub buses: Vec<ResolvedControl>,
}
impl PreparedControls {
    pub fn compile(session: &Session, maximum_bytes: usize) -> Result<Self, SessionError> {
        let required = session
            .inputs
            .len()
            .checked_add(session.sends.len())
            .and_then(|n| n.checked_add(session.buses.len()))
            .and_then(|n| n.checked_mul(size_of::<ResolvedControl>()))
            .and_then(|n| n.checked_add(size_of::<Self>()))
            .unwrap_or(usize::MAX);
        if required > maximum_bytes {
            return Err(SessionError::ResourceBudget {
                resource: "prepared controls",
                required,
                allowed: maximum_bytes,
            });
        }
        session.validate()?;
        let mut inputs = Vec::new();
        let mut sends = Vec::new();
        let mut buses = Vec::new();
        inputs
            .try_reserve_exact(session.inputs.len())
            .map_err(|_| SessionError::AllocationFailed)?;
        sends
            .try_reserve_exact(session.sends.len())
            .map_err(|_| SessionError::AllocationFailed)?;
        buses
            .try_reserve_exact(session.buses.len())
            .map_err(|_| SessionError::AllocationFailed)?;
        let input_solo = session.inputs.iter().any(|input| input.controls.solo);
        let bus_solo = session.buses.iter().any(|bus| bus.controls.solo);
        for input in &session.inputs {
            inputs.push(ResolvedControl {
                id: input.id,
                gains: input.controls.gains(),
                active: !input.controls.muted && (!input_solo || input.controls.solo),
            });
        }
        for send in &session.sends {
            sends.push(ResolvedControl {
                id: send.id,
                gains: send.controls.gains(),
                active: !send.controls.muted,
            });
        }
        // Resolve per-mix solo without a quadratic send-by-send search or a
        // temporary allocation. Empty mixes do not introduce a solo scope.
        for bus in &session.buses {
            if session
                .sends
                .iter()
                .any(|send| send.bus_id == bus.id && send.controls.solo)
            {
                for (source, resolved) in session.sends.iter().zip(&mut sends) {
                    if source.bus_id == bus.id {
                        resolved.active = !source.controls.muted && source.controls.solo;
                    }
                }
            }
        }
        for bus in &session.buses {
            buses.push(ResolvedControl {
                id: bus.id,
                gains: bus.controls.gains(),
                active: !bus.controls.muted && (!bus_solo || bus.controls.solo),
            });
        }
        Ok(Self {
            inputs,
            sends,
            buses,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlError {
    LayoutMismatch,
    InvalidGain,
}
impl fmt::Display for ControlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::LayoutMismatch => {
                "prepared controls do not match the compiled graph's stable IDs and order"
            }
            Self::InvalidGain => {
                "prepared controls contain a nonfinite, negative or excessive gain"
            }
        })
    }
}
impl Error for ControlError {}
