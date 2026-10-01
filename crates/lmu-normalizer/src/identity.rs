//! Session identity (vehicle, track, session type) and change detection → `SessionEvent`s.
//!
//! The identity keeps the **last known** value of each field: a field that is momentarily empty (menus,
//! loading) never overwrites a known value and never triggers an event. A change is reported when a field
//! becomes known for the first time or takes a different known value.

use telemetry_model::enums::{SessionType, VehicleClass};
use telemetry_model::events::SessionEvent;
use telemetry_model::scoring::SessionIdentity;

/// What changed in one observation.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct IdentityChange {
    pub session_changed: bool,
    pub track_changed: bool,
    pub vehicle_changed: bool,
    /// `SessionChanged` / `TrackChanged` / `VehicleChanged`, in that order.
    pub events: Vec<SessionEvent>,
}

impl IdentityChange {
    /// `true` if the session context (session, track or vehicle) changed.
    pub fn any(&self) -> bool {
        self.session_changed || self.track_changed || self.vehicle_changed
    }
}

/// Tracks the current [`SessionIdentity`].
#[derive(Debug, Clone, Default)]
pub struct IdentityTracker {
    current: SessionIdentity,
}

/// Replaces `slot` with `candidate` if the candidate is known and different; returns `true` on change.
fn merge<T: PartialEq + Clone>(slot: &mut Option<T>, candidate: &Option<T>) -> bool {
    match candidate {
        Some(new) if slot.as_ref() != Some(new) => {
            *slot = Some(new.clone());
            true
        }
        _ => false,
    }
}

impl IdentityTracker {
    /// The current (last known) identity.
    pub fn current(&self) -> &SessionIdentity {
        &self.current
    }

    /// Session type currently known.
    pub fn session_type(&self) -> Option<SessionType> {
        self.current.session_type
    }

    /// Merges `candidate` (unknown fields `None`) and reports what changed.
    pub fn observe(&mut self, candidate: &SessionIdentity) -> IdentityChange {
        let previous_session = self.current.session_type;
        let cur = &mut self.current;
        let session_changed = merge(&mut cur.session_type, &candidate.session_type);
        let track_changed = merge(&mut cur.track_name, &candidate.track_name);
        // The vehicle is identified by its telemetry name, model and class enum; evaluate all three.
        let name = merge(&mut cur.vehicle_name, &candidate.vehicle_name);
        let model = merge(&mut cur.vehicle_model, &candidate.vehicle_model);
        let class = merge::<VehicleClass>(&mut cur.vehicle_class, &candidate.vehicle_class);
        let vehicle_changed = name || model || class;
        // Scoring class string and game version are descriptive only.
        merge(&mut cur.class_name, &candidate.class_name);
        merge(&mut cur.game_version, &candidate.game_version);

        let mut events = Vec::new();
        if let (true, Some(to)) = (session_changed, cur.session_type) {
            events.push(SessionEvent::SessionChanged {
                from: previous_session,
                to,
            });
        }
        if track_changed {
            events.push(SessionEvent::TrackChanged {
                track_name: cur.track_name.clone(),
            });
        }
        if vehicle_changed {
            events.push(SessionEvent::VehicleChanged {
                vehicle_name: cur.vehicle_name.clone(),
                class_name: cur.class_name.clone(),
            });
        }
        IdentityChange {
            session_changed,
            track_changed,
            vehicle_changed,
            events,
        }
    }

    /// A restart of the current session (player ET went backwards): `SessionChanged` from and to the
    /// current type, if it is known.
    pub fn restart_event(&self) -> Option<SessionEvent> {
        self.current
            .session_type
            .map(|t| SessionEvent::SessionChanged {
                from: Some(t),
                to: t,
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ident(vehicle: Option<&str>, track: Option<&str>, session: Option<i32>) -> SessionIdentity {
        SessionIdentity {
            vehicle_name: vehicle.map(str::to_owned),
            track_name: track.map(str::to_owned),
            session_type: session.map(SessionType::from_lmu),
            ..SessionIdentity::default()
        }
    }

    #[test]
    fn first_observation_reports_everything_known() {
        let mut t = IdentityTracker::default();
        let c = t.observe(&ident(Some("Car A"), Some("Spa"), Some(1)));
        assert!(c.session_changed && c.track_changed && c.vehicle_changed);
        assert_eq!(c.events.len(), 3);
        assert_eq!(
            c.events[0],
            SessionEvent::SessionChanged {
                from: None,
                to: SessionType::from_lmu(1)
            }
        );
    }

    #[test]
    fn changes_are_detected_and_unknown_fields_ignored() {
        let mut t = IdentityTracker::default();
        t.observe(&ident(Some("Car A"), Some("Spa"), Some(1)));
        let same = t.observe(&ident(Some("Car A"), Some("Spa"), Some(1)));
        assert!(!same.any() && same.events.is_empty());
        let blank = t.observe(&ident(None, None, None));
        assert!(!blank.any(), "menus/loading never trigger changes");
        assert_eq!(t.current().vehicle_name.as_deref(), Some("Car A"));

        let veh = t.observe(&ident(Some("Car B"), Some("Spa"), Some(1)));
        assert!(veh.vehicle_changed && !veh.track_changed && !veh.session_changed);
        assert_eq!(
            veh.events,
            vec![SessionEvent::VehicleChanged {
                vehicle_name: Some("Car B".into()),
                class_name: None
            }]
        );
        let track = t.observe(&ident(Some("Car B"), Some("Le Mans"), Some(1)));
        assert_eq!(
            track.events,
            vec![SessionEvent::TrackChanged {
                track_name: Some("Le Mans".into())
            }]
        );
        let session = t.observe(&ident(Some("Car B"), Some("Le Mans"), Some(10)));
        assert_eq!(
            session.events,
            vec![SessionEvent::SessionChanged {
                from: Some(SessionType::from_lmu(1)),
                to: SessionType::from_lmu(10)
            }]
        );
        assert!(t.restart_event().is_some());
    }
}
