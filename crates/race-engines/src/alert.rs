//! Rule-based alert engine (spec §31, §70, §145 "alerts").
//!
//! # Rules
//! Each [`AlertRule`] watches one numeric channel (a canonical registry name such as
//! `fuel.laps_remaining`, or any key the caller resolves) and has a **trigger** threshold, a separate
//! **clear** threshold (hysteresis), a **minimum duration** the condition must hold before the alert is
//! raised, a **cooldown** after clearing during which it cannot be raised again, a severity and a category.
//!
//! # State machine (per rule)
//! ```text
//! Idle ──trigger crossed──▶ Pending ──held ≥ min duration──▶ Raised ─next tick─▶ Active
//!   ▲                          │ condition lost / value missing               │ clear crossed
//!   └──────────────────────────┘                                              ▼
//!   └────────────── cooldown elapsed ◀──────────────────────────────────── Cleared
//! ```
//! One continuous condition produces **one** alert (spec §31, §70): once raised, the alert stays active
//! until the value crosses the *clear* threshold, so a value flapping around the trigger inside the
//! hysteresis band produces no new alerts; after clearing, the cooldown suppresses immediate re-raising.
//! A missing value never raises or clears an alert: a pending condition is dropped (it cannot be confirmed)
//! and an active alert is held. An elapsed-time regression (session restart) drops pending conditions and
//! cooldowns.
//!
//! Every raise records an [`AnomalyRecord`] (time, lap, channel, measured value, threshold, severity).
//! Memory is bounded: at most [`MAX_RULES`] rules, [`MAX_HISTORY`] cleared alerts and [`MAX_ANOMALIES`]
//! anomaly records.

use std::collections::{HashSet, VecDeque};

use serde::{Deserialize, Serialize};
use telemetry_model::events::Severity;
use telemetry_model::{TelemetryFrame, ch, registry};
use thiserror::Error;
use tracing::{debug, info};

use crate::common::frame_f64;

/// Tracing component name.
const COMPONENT: &str = "race_engines::alert";

/// Most rules accepted in one configuration.
pub const MAX_RULES: usize = 256;
/// Cleared alerts kept.
pub const MAX_HISTORY: usize = 1_000;
/// Anomaly records kept.
pub const MAX_ANOMALIES: usize = 10_000;

/// Default fuel reserve (laps) below which the fuel alert triggers.
pub const DEFAULT_FUEL_RESERVE_LAPS: f64 = 2.0;
/// Default virtual-energy reserve (laps).
pub const DEFAULT_ENERGY_RESERVE_LAPS: f64 = 2.0;
/// Hysteresis above a reserve before a low-fuel / low-energy alert clears (laps).
pub const RESERVE_CLEAR_MARGIN_LAPS: f64 = 0.5;
/// Default maximum stream age before the network alert (s).
pub const DEFAULT_STREAM_AGE_MAX_S: f64 = 2.0;
/// Default maximum packet loss fraction before the network alert.
pub const DEFAULT_PACKET_LOSS_MAX_FRAC: f64 = 0.05;
/// Default maximum capture-to-publish latency before the data-quality alert (s).
pub const DEFAULT_PIPELINE_LATENCY_MAX_S: f64 = 0.5;
/// Fraction of a maximum at which an "above" default rule clears.
pub const DEFAULT_CLEAR_RATIO: f64 = 0.8;
/// Default hold time before raising (s).
pub const DEFAULT_MIN_DURATION_S: f64 = 1.0;
/// Default cooldown after clearing (s).
pub const DEFAULT_COOLDOWN_S: f64 = 30.0;

/// What an alert is about (spec §31).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AlertCategory {
    /// Fuel and energy.
    Fuel,
    /// Tyres.
    Tyres,
    /// Brakes.
    Brakes,
    /// Race-control flags.
    RaceFlags,
    /// Link to the relay / engineer.
    Network,
    /// Telemetry data quality.
    DataQuality,
}

/// Direction of a rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Comparison {
    /// Triggers when the value rises above `trigger`, clears below `clear` (`clear ≤ trigger`).
    Above,
    /// Triggers when the value falls below `trigger`, clears above `clear` (`clear ≥ trigger`).
    Below,
}

/// One alert rule.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AlertRule {
    /// Unique id.
    pub id: String,
    /// Watched channel (canonical registry name or caller-resolved key).
    pub channel: String,
    /// Category.
    pub category: AlertCategory,
    /// Severity.
    pub severity: Severity,
    /// Direction.
    pub comparison: Comparison,
    /// Trigger threshold (channel unit).
    pub trigger: f64,
    /// Clear threshold (channel unit), on the safe side of `trigger`.
    pub clear: f64,
    /// The condition must hold this long before raising (s, ≥ 0).
    pub min_duration_s: f64,
    /// After clearing, the rule cannot raise again for this long (s, ≥ 0).
    pub cooldown_s: f64,
    /// Human-readable message.
    #[serde(default)]
    pub message: String,
}

/// Invalid configuration.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum AlertConfigError {
    /// A reserve or maximum is negative or not finite.
    #[error("{field} must be a finite number ≥ 0, got {value}")]
    InvalidParameter {
        /// Field name.
        field: &'static str,
        /// Rejected value.
        value: f64,
    },
    /// A rule is malformed.
    #[error("rule {rule:?}: {reason}")]
    InvalidRule {
        /// Rule id.
        rule: String,
        /// Why.
        reason: &'static str,
    },
    /// Two rules share an id.
    #[error("duplicate rule id {0:?}")]
    DuplicateRule(String),
    /// Too many rules.
    #[error("{got} rules exceed the maximum of {max}")]
    TooManyRules {
        /// Number given.
        got: usize,
        /// Limit.
        max: usize,
    },
}

/// Engine errors.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum AlertError {
    /// No active alert with this id.
    #[error("no active alert with id {0}")]
    UnknownAlert(u64),
}

impl AlertRule {
    /// Validates the rule on its own.
    pub fn validate(&self) -> Result<(), AlertConfigError> {
        let err = |reason| AlertConfigError::InvalidRule {
            rule: self.id.clone(),
            reason,
        };
        if self.id.trim().is_empty() {
            return Err(err("empty id"));
        }
        if self.channel.trim().is_empty() {
            return Err(err("empty channel"));
        }
        if !self.trigger.is_finite() || !self.clear.is_finite() {
            return Err(err("thresholds must be finite"));
        }
        match self.comparison {
            Comparison::Above if self.clear > self.trigger => {
                return Err(err("an Above rule needs clear <= trigger"));
            }
            Comparison::Below if self.clear < self.trigger => {
                return Err(err("a Below rule needs clear >= trigger"));
            }
            _ => {}
        }
        if !(self.min_duration_s.is_finite() && self.min_duration_s >= 0.0) {
            return Err(err("min_duration_s must be finite and >= 0"));
        }
        if !(self.cooldown_s.is_finite() && self.cooldown_s >= 0.0) {
            return Err(err("cooldown_s must be finite and >= 0"));
        }
        Ok(())
    }

    fn triggered(&self, v: f64) -> bool {
        match self.comparison {
            Comparison::Above => v > self.trigger,
            Comparison::Below => v < self.trigger,
        }
    }

    fn cleared(&self, v: f64) -> bool {
        match self.comparison {
            Comparison::Above => v < self.clear,
            Comparison::Below => v > self.clear,
        }
    }
}

/// Alert configuration: parameters of the default rule set plus custom rules.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AlertConfig {
    /// Include the default rules built from the parameters below.
    pub include_default_rules: bool,
    /// Fuel reserve (laps, ≥ 0): alert when fuel laps remaining fall below it.
    pub fuel_reserve_laps: f64,
    /// Virtual-energy reserve (laps, ≥ 0).
    pub energy_reserve_laps: f64,
    /// Maximum stream age before the network alert (s, > 0).
    pub stream_age_max_s: f64,
    /// Maximum packet loss before the network alert (fraction, > 0).
    pub packet_loss_max_frac: f64,
    /// Maximum capture-to-publish latency before the data-quality alert (s, > 0).
    pub pipeline_latency_max_s: f64,
    /// Engineer-defined rules (tyre, brake, flag thresholds have no universal default).
    pub rules: Vec<AlertRule>,
}

impl Default for AlertConfig {
    fn default() -> Self {
        Self {
            include_default_rules: true,
            fuel_reserve_laps: DEFAULT_FUEL_RESERVE_LAPS,
            energy_reserve_laps: DEFAULT_ENERGY_RESERVE_LAPS,
            stream_age_max_s: DEFAULT_STREAM_AGE_MAX_S,
            packet_loss_max_frac: DEFAULT_PACKET_LOSS_MAX_FRAC,
            pipeline_latency_max_s: DEFAULT_PIPELINE_LATENCY_MAX_S,
            rules: Vec::new(),
        }
    }
}

fn channel_name(id: telemetry_model::ChannelId) -> String {
    registry::by_id(id).map_or_else(String::new, |d| d.name.to_owned())
}

impl AlertConfig {
    /// Validates parameters and rules.
    pub fn validate(&self) -> Result<(), AlertConfigError> {
        for (field, value, allow_zero) in [
            ("fuel_reserve_laps", self.fuel_reserve_laps, true),
            ("energy_reserve_laps", self.energy_reserve_laps, true),
            ("stream_age_max_s", self.stream_age_max_s, false),
            ("packet_loss_max_frac", self.packet_loss_max_frac, false),
            ("pipeline_latency_max_s", self.pipeline_latency_max_s, false),
        ] {
            let ok = value.is_finite()
                && if allow_zero {
                    value >= 0.0
                } else {
                    value > 0.0
                };
            if !ok {
                return Err(AlertConfigError::InvalidParameter { field, value });
            }
        }
        let rules = self.effective_rules();
        if rules.len() > MAX_RULES {
            return Err(AlertConfigError::TooManyRules {
                got: rules.len(),
                max: MAX_RULES,
            });
        }
        let mut ids = HashSet::new();
        for r in &rules {
            r.validate()?;
            if !ids.insert(r.id.as_str()) {
                return Err(AlertConfigError::DuplicateRule(r.id.clone()));
            }
        }
        Ok(())
    }

    /// The default rules (when enabled) followed by the custom rules.
    pub fn effective_rules(&self) -> Vec<AlertRule> {
        let mut out = if self.include_default_rules {
            self.default_rules()
        } else {
            Vec::new()
        };
        out.extend(self.rules.iter().cloned());
        out
    }

    /// Default rule set from the parameters: low fuel, low virtual energy, stale stream, packet loss,
    /// pipeline latency. Tyre, brake and flag rules have no universal thresholds and are left to the
    /// engineer.
    ///
    /// Every rule watches a registry channel that some stage writes into the frame: `fuel.laps_remaining`
    /// by [`crate::publish::publish`], `energy.ve_laps_remaining` by [`crate::energy::publish_energy`]
    /// (the pipeline must call both), the network / latency channels by the transport layer. A frame
    /// without the channel never raises the alert (a missing value is not a condition).
    pub fn default_rules(&self) -> Vec<AlertRule> {
        let rule = |id: &str,
                    channel: String,
                    category,
                    severity,
                    comparison,
                    trigger: f64,
                    clear: f64,
                    message: &str| AlertRule {
            id: id.to_owned(),
            channel,
            category,
            severity,
            comparison,
            trigger,
            clear,
            min_duration_s: DEFAULT_MIN_DURATION_S,
            cooldown_s: DEFAULT_COOLDOWN_S,
            message: message.to_owned(),
        };
        vec![
            rule(
                "fuel_low",
                channel_name(ch::FUEL_LAPS_REMAINING),
                AlertCategory::Fuel,
                Severity::Warning,
                Comparison::Below,
                self.fuel_reserve_laps,
                self.fuel_reserve_laps + RESERVE_CLEAR_MARGIN_LAPS,
                "Fuel laps remaining below reserve",
            ),
            rule(
                "energy_low",
                channel_name(ch::VE_LAPS_REMAINING),
                AlertCategory::Fuel,
                Severity::Warning,
                Comparison::Below,
                self.energy_reserve_laps,
                self.energy_reserve_laps + RESERVE_CLEAR_MARGIN_LAPS,
                "Virtual energy laps remaining below reserve",
            ),
            rule(
                "stream_stale",
                channel_name(ch::NET_STREAM_AGE),
                AlertCategory::Network,
                Severity::Critical,
                Comparison::Above,
                self.stream_age_max_s,
                self.stream_age_max_s * DEFAULT_CLEAR_RATIO,
                "Telemetry stream is stale",
            ),
            rule(
                "packet_loss",
                channel_name(ch::NET_PACKET_LOSS),
                AlertCategory::Network,
                Severity::Warning,
                Comparison::Above,
                self.packet_loss_max_frac,
                self.packet_loss_max_frac * DEFAULT_CLEAR_RATIO,
                "Packet loss high",
            ),
            rule(
                "pipeline_latency",
                channel_name(ch::PIPELINE_LATENCY),
                AlertCategory::DataQuality,
                Severity::Notice,
                Comparison::Above,
                self.pipeline_latency_max_s,
                self.pipeline_latency_max_s * DEFAULT_CLEAR_RATIO,
                "Capture-to-publish latency high",
            ),
        ]
    }
}

/// Lifecycle state of an alert.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AlertState {
    /// Raised on this evaluation.
    Raised,
    /// Still active (condition not yet cleared).
    Active,
    /// Cleared (value crossed the clear threshold).
    Cleared,
}

/// One alert instance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Alert {
    /// Engine-unique id (monotonic).
    pub id: u64,
    /// Rule id.
    pub rule_id: String,
    /// Category.
    pub category: AlertCategory,
    /// Severity.
    pub severity: Severity,
    /// Watched channel.
    pub channel: String,
    /// Rule message.
    pub message: String,
    /// Lifecycle state.
    pub state: AlertState,
    /// Engineer acknowledged it.
    pub acknowledged: bool,
    /// Time the condition started (s, LMU elapsed time).
    pub condition_since_s: f64,
    /// Time raised (s).
    pub raised_s: f64,
    /// Lap when raised.
    pub raised_lap: Option<i32>,
    /// Time cleared (s).
    pub cleared_s: Option<f64>,
    /// Value when raised.
    pub raised_value: f64,
    /// Most extreme value while active (highest for Above, lowest for Below).
    pub worst_value: f64,
    /// Trigger threshold.
    pub threshold: f64,
}

/// Anomaly event record created when an alert is raised.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnomalyRecord {
    /// Alert id.
    pub alert_id: u64,
    /// Rule id.
    pub rule_id: String,
    /// Category.
    pub category: AlertCategory,
    /// Severity.
    pub severity: Severity,
    /// Time (s, LMU elapsed time).
    pub et_s: f64,
    /// Lap.
    pub lap: Option<i32>,
    /// Channel.
    pub channel: String,
    /// Measured value.
    pub measured: f64,
    /// Threshold crossed.
    pub threshold: f64,
}

/// A state change reported by [`AlertEngine::evaluate`].
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AlertNotice {
    /// New state (`Raised` or `Cleared`).
    pub state: AlertState,
    /// The alert after the change.
    pub alert: Alert,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Phase {
    Idle,
    Pending { since: f64 },
    Active { index: usize },
    Cooldown { until: f64 },
}

/// Deterministic alert engine.
#[derive(Debug, Clone)]
pub struct AlertEngine {
    rules: Vec<AlertRule>,
    phases: Vec<Phase>,
    active: Vec<Alert>,
    history: VecDeque<Alert>,
    anomalies: VecDeque<AnomalyRecord>,
    next_id: u64,
    last_et: Option<f64>,
}

impl AlertEngine {
    /// New engine from a validated configuration.
    pub fn new(config: &AlertConfig) -> Result<Self, AlertConfigError> {
        config.validate()?;
        let rules = config.effective_rules();
        Ok(Self {
            phases: vec![Phase::Idle; rules.len()],
            rules,
            active: Vec::new(),
            history: VecDeque::new(),
            anomalies: VecDeque::new(),
            next_id: 1,
            last_et: None,
        })
    }

    /// Rules in evaluation order.
    pub fn rules(&self) -> &[AlertRule] {
        &self.rules
    }

    /// Active alerts (raised, not yet cleared).
    pub fn active(&self) -> &[Alert] {
        &self.active
    }

    /// Cleared alerts, oldest first.
    pub fn history(&self) -> impl Iterator<Item = &Alert> {
        self.history.iter()
    }

    /// Anomaly records, oldest first.
    pub fn anomalies(&self) -> impl Iterator<Item = &AnomalyRecord> {
        self.anomalies.iter()
    }

    /// Acknowledges an active alert.
    pub fn acknowledge(&mut self, alert_id: u64) -> Result<(), AlertError> {
        let a = self
            .active
            .iter_mut()
            .find(|a| a.id == alert_id)
            .ok_or(AlertError::UnknownAlert(alert_id))?;
        a.acknowledged = true;
        debug!(component = COMPONENT, alert_id, "alert acknowledged");
        Ok(())
    }

    /// Evaluates every rule against a canonical frame (channels resolved by registry name; unknown names
    /// or invalid samples are missing values).
    pub fn evaluate_frame(
        &mut self,
        et_s: f64,
        lap: Option<i32>,
        frame: &TelemetryFrame,
    ) -> Vec<AlertNotice> {
        self.evaluate(et_s, lap, |name| {
            registry::by_name(name).and_then(|d| frame_f64(frame, d.id))
        })
    }

    /// Evaluates every rule at time `et_s`. `value_of` returns the current value of a channel, `None` when
    /// unavailable. Returns the alerts raised or cleared by this evaluation.
    pub fn evaluate(
        &mut self,
        et_s: f64,
        lap: Option<i32>,
        value_of: impl Fn(&str) -> Option<f64>,
    ) -> Vec<AlertNotice> {
        let mut notices = Vec::new();
        if !et_s.is_finite() {
            return notices;
        }
        if self.last_et.is_some_and(|t| et_s < t) {
            debug!(
                component = COMPONENT,
                et_s, "time regression: pending and cooldowns dropped"
            );
            for p in &mut self.phases {
                if matches!(p, Phase::Pending { .. } | Phase::Cooldown { .. }) {
                    *p = Phase::Idle;
                }
            }
        }
        self.last_et = Some(et_s);
        // Alerts raised on the previous evaluation become Active.
        for a in &mut self.active {
            if a.state == AlertState::Raised {
                a.state = AlertState::Active;
            }
        }
        let mut to_clear = Vec::new();
        for i in 0..self.rules.len() {
            let value = value_of(&self.rules[i].channel).filter(|v| v.is_finite());
            if let Phase::Cooldown { until } = self.phases[i]
                && et_s >= until
            {
                self.phases[i] = Phase::Idle;
            }
            let rule = &self.rules[i];
            match (self.phases[i], value) {
                (Phase::Idle, Some(v)) if rule.triggered(v) => {
                    self.phases[i] = Phase::Pending { since: et_s };
                }
                (Phase::Pending { .. }, Some(v)) if !rule.triggered(v) => {
                    self.phases[i] = Phase::Idle;
                }
                (Phase::Pending { .. }, None) => self.phases[i] = Phase::Idle,
                (Phase::Active { index }, Some(v)) => {
                    if let Some(a) = self.active.get_mut(index) {
                        a.worst_value = match rule.comparison {
                            Comparison::Above => a.worst_value.max(v),
                            Comparison::Below => a.worst_value.min(v),
                        };
                    }
                    if rule.cleared(v) {
                        to_clear.push(i);
                    }
                }
                _ => {}
            }
            if let (Phase::Pending { since }, Some(v)) = (self.phases[i], value)
                && et_s - since >= self.rules[i].min_duration_s
            {
                let index = self.raise(i, since, et_s, lap, v);
                self.phases[i] = Phase::Active { index };
                notices.push(AlertNotice {
                    state: AlertState::Raised,
                    alert: self.active[index].clone(),
                });
            }
        }
        for i in to_clear {
            if let Some(n) = self.clear(i, et_s) {
                notices.push(n);
            }
        }
        notices
    }

    fn raise(&mut self, rule_idx: usize, since: f64, et_s: f64, lap: Option<i32>, v: f64) -> usize {
        let r = &self.rules[rule_idx];
        let id = self.next_id;
        self.next_id += 1;
        info!(component = COMPONENT, rule = %r.id, alert_id = id, value = v, threshold = r.trigger, "alert raised");
        self.active.push(Alert {
            id,
            rule_id: r.id.clone(),
            category: r.category,
            severity: r.severity,
            channel: r.channel.clone(),
            message: r.message.clone(),
            state: AlertState::Raised,
            acknowledged: false,
            condition_since_s: since,
            raised_s: et_s,
            raised_lap: lap,
            cleared_s: None,
            raised_value: v,
            worst_value: v,
            threshold: r.trigger,
        });
        self.anomalies.push_back(AnomalyRecord {
            alert_id: id,
            rule_id: r.id.clone(),
            category: r.category,
            severity: r.severity,
            et_s,
            lap,
            channel: r.channel.clone(),
            measured: v,
            threshold: r.trigger,
        });
        while self.anomalies.len() > MAX_ANOMALIES {
            self.anomalies.pop_front();
        }
        self.active.len() - 1
    }

    fn clear(&mut self, rule_idx: usize, et_s: f64) -> Option<AlertNotice> {
        let Phase::Active { index } = self.phases[rule_idx] else {
            return None;
        };
        if index >= self.active.len() {
            self.phases[rule_idx] = Phase::Idle;
            return None;
        }
        let mut alert = self.active.remove(index);
        // Removing shifts later active alerts down by one.
        for p in &mut self.phases {
            if let Phase::Active { index: j } = p
                && *j > index
            {
                *j -= 1;
            }
        }
        alert.state = AlertState::Cleared;
        alert.cleared_s = Some(et_s);
        info!(component = COMPONENT, rule = %alert.rule_id, alert_id = alert.id, "alert cleared");
        self.phases[rule_idx] = Phase::Cooldown {
            until: et_s + self.rules[rule_idx].cooldown_s,
        };
        self.history.push_back(alert.clone());
        while self.history.len() > MAX_HISTORY {
            self.history.pop_front();
        }
        Some(AlertNotice {
            state: AlertState::Cleared,
            alert,
        })
    }
}

#[cfg(test)]
mod tests;
