//! Pure control-thread validation of explicit output routes. No host is created.

use crate::OutputId;
use relink_core::{Session, SessionError, Uuid};
use std::{collections::HashMap, collections::HashSet, error::Error, fmt};

/// One bus sent to one explicitly identified native output endpoint.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutputRoute {
    pub output: OutputId,
    pub bus_id: Uuid,
}

/// Simultaneous-output configuration; this type does not start any streams.
///
/// The selected clock endpoint owns graph rendering. Route order is preserved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutputPlan {
    pub clock_output: OutputId,
    pub routes: Vec<OutputRoute>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OutputPlanError {
    InvalidSession(SessionError),
    NoOutputs,
    UnsupportedPlatform,
    InvalidOutputId {
        output: OutputId,
        reason: &'static str,
    },
    UnknownBus {
        route_index: usize,
        bus_id: Uuid,
    },
    DuplicateOutput {
        output: OutputId,
        first_route: usize,
        duplicate_route: usize,
    },
    ClockOutputMissing(OutputId),
}

impl fmt::Display for OutputPlanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSession(error) => write!(f, "Cannot route this session: {error}"),
            Self::NoOutputs => f.write_str("Select at least one output route."),
            Self::UnsupportedPlatform => {
                f.write_str("Output plans support macOS/CoreAudio and Windows/WASAPI.")
            }
            Self::InvalidOutputId { output, reason } => {
                write!(f, "Invalid output identity {:?}: {reason}", output.0)
            }
            Self::UnknownBus {
                route_index,
                bus_id,
            } => write!(
                f,
                "Output route {} refers to missing mix {bus_id}.",
                route_index + 1
            ),
            Self::DuplicateOutput {
                output,
                first_route,
                duplicate_route,
            } => write!(
                f,
                "Output routes {} and {} select the same endpoint {:?}; select each endpoint once.",
                first_route + 1,
                duplicate_route + 1,
                output.0
            ),
            Self::ClockOutputMissing(output) => write!(
                f,
                "The selected clock output {:?} must appear in the output routes.",
                output.0
            ),
        }
    }
}

impl Error for OutputPlanError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InvalidSession(error) => Some(error),
            _ => None,
        }
    }
}

impl OutputPlan {
    /// Construct the one-route equivalent of the existing single-output API.
    /// Validation is explicit and does not occur during construction.
    pub fn single(output: OutputId, bus_id: Uuid) -> Self {
        Self {
            clock_output: output.clone(),
            routes: vec![OutputRoute { output, bus_id }],
        }
    }

    /// Validate once on the control thread, before any device discovery, and
    /// return the clock route's index without modifying or reordering the plan.
    ///
    /// Checks the complete core session, bus references and parsed native endpoint
    /// identities. Equal parsed endpoint IDs cannot occur twice, even for different
    /// buses. Distinct endpoints can receive the same bus. This does not establish
    /// whether two endpoints ultimately share physical hardware.
    ///
    /// Host spelling must match CPAL's persisted representation. The native suffix
    /// is opaque and preserved exactly, including case, whitespace and colons.
    /// An opaque suffix such as `default` is a literal ID, never a default-device
    /// alias. No host, device or default-device API is invoked here.
    ///
    /// Syntax validation does not prove device existence. Input-backend support,
    /// availability, exact nominal rate, layout and native activation remain
    /// subsequent control-thread/OS checks before program audio can be enabled.
    pub fn validate(&self, session: &Session) -> Result<usize, OutputPlanError> {
        session
            .validate()
            .map_err(OutputPlanError::InvalidSession)?;
        if self.routes.is_empty() {
            return Err(OutputPlanError::NoOutputs);
        }
        let host = supported_host()?;
        let clock = parse_output(&self.clock_output, host)?;
        let buses: HashSet<_> = session.buses.iter().map(|bus| bus.id).collect();
        let mut endpoints = HashMap::new();
        let mut clock_index = None;
        for (index, route) in self.routes.iter().enumerate() {
            if !buses.contains(&route.bus_id) {
                return Err(OutputPlanError::UnknownBus {
                    route_index: index,
                    bus_id: route.bus_id,
                });
            }
            let endpoint = parse_output(&route.output, host)?;
            if endpoint == clock {
                clock_index = Some(index);
            }
            if let Some(first_route) = endpoints.insert(endpoint, index) {
                return Err(OutputPlanError::DuplicateOutput {
                    output: route.output.clone(),
                    first_route,
                    duplicate_route: index,
                });
            }
        }
        clock_index.ok_or_else(|| OutputPlanError::ClockOutputMissing(self.clock_output.clone()))
    }
}

fn supported_host() -> Result<cpal::HostId, OutputPlanError> {
    #[cfg(target_os = "macos")]
    {
        Ok(cpal::HostId::CoreAudio)
    }
    #[cfg(target_os = "windows")]
    {
        Ok(cpal::HostId::Wasapi)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        Err(OutputPlanError::UnsupportedPlatform)
    }
}

fn parse_output(output: &OutputId, host: cpal::HostId) -> Result<cpal::DeviceId, OutputPlanError> {
    let invalid = |reason| OutputPlanError::InvalidOutputId {
        output: output.clone(),
        reason,
    };
    let parsed = output
        .0
        .parse::<cpal::DeviceId>()
        .map_err(|_| invalid("expected a host-qualified native ID for this platform"))?;
    if parsed.host() != host {
        return Err(invalid("the host is not the supported output backend"));
    }
    if parsed.id().is_empty() {
        return Err(invalid("the native endpoint ID cannot be empty"));
    }
    // CPAL accepts case-insensitive host aliases. Requiring its serialized form
    // prevents silently rewriting an imported identity; the native part is opaque.
    if parsed.to_string() != output.0 {
        return Err(invalid(
            "use the canonical host-qualified ID returned by device discovery",
        ));
    }
    Ok(parsed)
}

#[cfg(all(test, any(target_os = "macos", target_os = "windows")))]
mod tests {
    use super::*;
    use relink_core::{Bus, Input, InputKind};

    fn output(native: &str) -> OutputId {
        OutputId(format!("{}:{native}", supported_host().unwrap()))
    }

    fn session() -> (Session, Uuid, Uuid) {
        let mut session = Session::new("Plan controls", 48_000);
        let first = session.add_bus(Bus::new("First")).unwrap();
        let second = session.add_bus(Bus::new("Second")).unwrap();
        (session, first, second)
    }

    #[test]
    fn single_plan_selects_exact_endpoint_without_discovery() {
        let (session, bus, _) = session();
        let id = output("endpoint-that-does-not-need-to-exist");
        let plan = OutputPlan::single(id.clone(), bus);
        assert_eq!(plan.clock_output, id);
        assert_eq!(
            plan.routes,
            vec![OutputRoute {
                output: id,
                bus_id: bus
            }]
        );
        assert_eq!(plan.validate(&session), Ok(0));
    }

    #[test]
    fn multiple_endpoints_can_share_a_bus_and_keep_order() {
        let (session, bus, _) = session();
        let plan = OutputPlan {
            clock_output: output("middle"),
            routes: ["last", "middle", "first"]
                .map(|native| OutputRoute {
                    output: output(native),
                    bus_id: bus,
                })
                .into(),
        };
        let before = plan.clone();
        assert_eq!(plan.validate(&session), Ok(1));
        assert_eq!(plan, before);
    }

    #[test]
    fn duplicate_endpoint_is_rejected_even_for_different_buses() {
        let (session, first, second) = session();
        let mut plan = OutputPlan::single(output("same"), first);
        plan.routes.push(OutputRoute {
            output: output("same"),
            bus_id: second,
        });
        assert_eq!(
            plan.validate(&session),
            Err(OutputPlanError::DuplicateOutput {
                output: output("same"),
                first_route: 0,
                duplicate_route: 1,
            })
        );
    }

    #[test]
    fn every_route_must_reference_an_existing_bus() {
        let (session, bus, _) = session();
        let mut plan = OutputPlan::single(output("clock"), bus);
        let unknown = Uuid::new_v4();
        plan.routes.push(OutputRoute {
            output: output("follower"),
            bus_id: unknown,
        });
        assert_eq!(
            plan.validate(&session),
            Err(OutputPlanError::UnknownBus {
                route_index: 1,
                bus_id: unknown,
            })
        );
    }

    #[test]
    fn missing_clock_and_empty_plan_are_explicit_errors() {
        let (session, bus, _) = session();
        let mut plan = OutputPlan::single(output("present"), bus);
        plan.clock_output = output("absent");
        assert_eq!(
            plan.validate(&session),
            Err(OutputPlanError::ClockOutputMissing(output("absent")))
        );
        plan.routes.clear();
        assert_eq!(plan.validate(&session), Err(OutputPlanError::NoOutputs));
    }

    #[test]
    fn malformed_empty_wrong_host_and_noncanonical_ids_are_rejected() {
        let (session, bus, _) = session();
        let invalid = [
            "".to_owned(),
            "unqualified-native-id".to_owned(),
            ":native".to_owned(),
            "unknown:native".to_owned(),
            format!("{}:", supported_host().unwrap()),
            format!("{}:native", supported_host().unwrap()).to_uppercase(),
            format!(" {}:native", supported_host().unwrap()),
            if cfg!(target_os = "macos") {
                "wasapi:native"
            } else {
                "coreaudio:native"
            }
            .to_owned(),
        ];
        for text in invalid {
            let id = OutputId(text);
            let plan = OutputPlan::single(id.clone(), bus);
            assert!(
                matches!(plan.validate(&session), Err(OutputPlanError::InvalidOutputId { output, .. }) if output == id)
            );
            let plan = OutputPlan {
                clock_output: output("valid"),
                routes: vec![OutputRoute {
                    output: id.clone(),
                    bus_id: bus,
                }],
            };
            assert!(
                matches!(plan.validate(&session), Err(OutputPlanError::InvalidOutputId { output, .. }) if output == id)
            );
        }
    }

    #[test]
    fn opaque_native_suffix_is_neither_normalized_nor_a_default_alias() {
        let (session, bus, _) = session();
        let plan = OutputPlan {
            clock_output: output("default"),
            routes: ["default", " Native:Case ", " Native:case "]
                .map(|native| OutputRoute {
                    output: output(native),
                    bus_id: bus,
                })
                .into(),
        };
        let before = plan.clone();
        assert_eq!(plan.validate(&session), Ok(0));
        assert_eq!(plan, before);
        let mut absent = plan.clone();
        absent.clock_output = output("system-default");
        assert_eq!(
            absent.validate(&session),
            Err(OutputPlanError::ClockOutputMissing(output(
                "system-default"
            )))
        );
    }

    #[test]
    fn validates_the_full_session_including_unrouted_bus_and_input() {
        let (mut session, bus, _) = session();
        let plan = OutputPlan::single(output("valid"), bus);
        session.buses[1].controls.gain_db = f32::NAN;
        assert!(matches!(
            plan.validate(&session),
            Err(OutputPlanError::InvalidSession(_))
        ));
        session.buses[1].controls.gain_db = 0.0;
        session.inputs.push(Input::new(
            "Invalid tone",
            InputKind::TestTone {
                frequency_hz: 99_000.0,
                amplitude_db: -12.0,
            },
        ));
        let error = plan.validate(&session).unwrap_err();
        assert!(matches!(error, OutputPlanError::InvalidSession(_)));
        assert!(error.source().is_some());
    }
}
