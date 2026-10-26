//! Saved, explicitly selected output routes. Incomplete drafts may be saved;
//! only a validated plan can start audio. Device refresh never rewrites IDs.
use ravelink_audio::{OutputId, OutputPlan, OutputRoute};
use ravelink_core::Session;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use uuid::Uuid;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputDraft {
    pub device_id: Option<String>,
    pub bus_id: Option<Uuid>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoutingDraft {
    pub outputs: Vec<OutputDraft>,
    pub clock_output: Option<String>,
}

impl RoutingDraft {
    /// Lightweight editor feedback. Full session/native-ID validation happens
    /// only on Start, not on every immediate-mode UI frame.
    pub fn issue(&self, session: &Session) -> Option<String> {
        if self.outputs.is_empty() {
            return Some("Add an output device before starting audio.".into());
        }
        let buses: HashSet<_> = session.buses.iter().map(|bus| bus.id).collect();
        let mut devices = HashSet::with_capacity(self.outputs.len());
        for (index, route) in self.outputs.iter().enumerate() {
            let Some(device) = &route.device_id else {
                return Some(format!("Choose a device for output {}.", index + 1));
            };
            if !devices.insert(device) {
                return Some("Each device can receive one mix. Choose a different device or remove the duplicate output.".into());
            }
            if !route.bus_id.is_some_and(|id| buses.contains(&id)) {
                return Some(format!("Choose an existing mix for output {}.", index + 1));
            }
        }
        if !self
            .outputs
            .iter()
            .any(|route| route.device_id.is_some() && route.device_id == self.clock_output)
        {
            return Some("Choose one clock device.".into());
        }
        None
    }

    pub fn plan(&self, session: &Session) -> Result<OutputPlan, String> {
        if self.outputs.is_empty() {
            return Err("Add an output device in Outputs before starting audio.".into());
        }
        let routes =
            self.outputs
                .iter()
                .enumerate()
                .map(|(index, draft)| {
                    Ok(OutputRoute {
                        output: OutputId(
                            draft.device_id.clone().ok_or_else(|| {
                                format!("Choose a device for output {}.", index + 1)
                            })?,
                        ),
                        bus_id: draft
                            .bus_id
                            .ok_or_else(|| format!("Choose a mix for output {}.", index + 1))?,
                    })
                })
                .collect::<Result<Vec<_>, String>>()?;
        let plan = OutputPlan {
            clock_output: OutputId(
                self.clock_output
                    .clone()
                    .ok_or("Choose one clock device in Outputs.")?,
            ),
            routes,
        };
        plan.validate(session).map_err(|e| e.to_string())?;
        Ok(plan)
    }

    pub fn remove(&mut self, index: usize) {
        if index >= self.outputs.len() {
            return;
        }
        let removed = self.outputs.remove(index);
        if removed.device_id.is_some() && removed.device_id == self.clock_output {
            self.clock_output = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ravelink_core::Bus;

    fn fixture() -> (Session, RoutingDraft) {
        let mut session = Session::new("Routes", 48_000);
        session.buses = vec![Bus::new("Monitor"), Bus::new("Stream")];
        #[cfg(target_os = "macos")]
        let host = "coreaudio";
        #[cfg(target_os = "windows")]
        let host = "wasapi";
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        let host = "unsupported";
        let first = format!("{host}:first");
        let second = format!("{host}:second");
        let draft = RoutingDraft {
            outputs: vec![
                OutputDraft {
                    device_id: Some(first),
                    bus_id: Some(session.buses[0].id),
                },
                OutputDraft {
                    device_id: Some(second.clone()),
                    bus_id: Some(session.buses[1].id),
                },
            ],
            clock_output: Some(second),
        };
        (session, draft)
    }

    #[test]
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    fn saved_routes_keep_order_clock_and_mix_independent_of_editor_selection() {
        let (mut session, draft) = fixture();
        let plan = draft.plan(&session).unwrap();
        assert_eq!(plan.validate(&session).unwrap(), 1);
        session.buses.reverse();
        session.buses[0].name = "Renamed".into();
        assert_eq!(draft.plan(&session).unwrap(), plan);
        session.buses.pop();
        assert!(
            draft.plan(&session).is_err(),
            "Missing mixes cannot silently route elsewhere"
        );
    }

    #[test]
    fn removing_clock_requires_an_explicit_replacement() {
        let (session, mut draft) = fixture();
        draft.remove(1);
        assert!(draft.clock_output.is_none());
        assert!(draft.plan(&session).is_err());
        assert_eq!(draft.outputs.len(), 1);
    }

    #[test]
    fn duplicate_or_incomplete_routes_never_start() {
        let (session, mut draft) = fixture();
        draft.outputs[1].device_id = draft.outputs[0].device_id.clone();
        draft.clock_output = draft.outputs[0].device_id.clone();
        assert!(draft.plan(&session).is_err());
        draft.outputs[0].bus_id = None;
        assert!(draft.plan(&session).is_err());
        assert!(RoutingDraft::default().plan(&session).is_err());
    }
}
