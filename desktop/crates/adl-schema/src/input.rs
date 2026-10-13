//! High-level input actions (PRD sections 5 and 6). The broker validates and expands these into
//! primitive adapter events, checking cancellation between every event.

use crate::coords::Point;
use crate::keys::{Key, KeyCombo};
use crate::limits::*;
use crate::{AdlError, Result};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Button {
    Left,
    Right,
    Middle,
    /// Optional, individually advertised.
    Back,
    Forward,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Axis {
    Vertical,
    Horizontal,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Action {
    /// Absolute pointer movement.
    Move { to: Point },
    /// Relative pointer movement in logical pixels, where the backend supports it.
    MoveRelative { dx: f64, dy: f64 },
    Click {
        #[serde(default = "left")]
        button: Button,
        /// 1 = single, 2 = double, 3 = triple.
        #[serde(default = "one")]
        count: u8,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        at: Option<Point>,
        /// Modifiers held for the duration of the click.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        modifiers: Vec<crate::keys::Modifier>,
    },
    ButtonDown { button: Button },
    ButtonUp { button: Button },
    /// Press, move along `steps` interpolated points, release.
    Drag {
        from: Point,
        to: Point,
        #[serde(default = "left")]
        button: Button,
        #[serde(default = "drag_steps")]
        steps: u32,
    },
    /// Scroll in discrete wheel steps (positive = down/right).
    Scroll {
        axis: Axis,
        steps: i32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        at: Option<Point>,
    },
    /// Move to a point and wait.
    Hover { to: Point, ms: u64 },
    /// Insert text. Never logged.
    Type { text: String },
    KeyPress { key: Key },
    KeyDown { key: Key },
    KeyUp { key: Key },
    Shortcut { keys: KeyCombo },
    /// Bounded pause between actions; still a cancellation checkpoint.
    Wait { ms: u64 },
}

fn left() -> Button {
    Button::Left
}
fn one() -> u8 {
    1
}
fn drag_steps() -> u32 {
    20
}

impl Action {
    pub fn is_keyboard(&self) -> bool {
        matches!(self, Action::Type { .. } | Action::KeyPress { .. } | Action::KeyDown { .. } | Action::KeyUp { .. } | Action::Shortcut { .. })
    }
    pub fn is_pointer(&self) -> bool {
        !self.is_keyboard() && !matches!(self, Action::Wait { .. })
    }
    /// Points this action references, for target and staleness validation.
    pub fn points(&self) -> Vec<&Point> {
        match self {
            Action::Move { to } | Action::Hover { to, .. } => vec![to],
            Action::Click { at, .. } | Action::Scroll { at, .. } => at.iter().collect(),
            Action::Drag { from, to, .. } => vec![from, to],
            _ => vec![],
        }
    }
    /// Audit-safe name: never includes typed text.
    pub fn kind(&self) -> &'static str {
        match self {
            Action::Move { .. } => "move",
            Action::MoveRelative { .. } => "move_relative",
            Action::Click { .. } => "click",
            Action::ButtonDown { .. } => "button_down",
            Action::ButtonUp { .. } => "button_up",
            Action::Drag { .. } => "drag",
            Action::Scroll { .. } => "scroll",
            Action::Hover { .. } => "hover",
            Action::Type { .. } => "type",
            Action::KeyPress { .. } => "key_press",
            Action::KeyDown { .. } => "key_down",
            Action::KeyUp { .. } => "key_up",
            Action::Shortcut { .. } => "shortcut",
            Action::Wait { .. } => "wait",
        }
    }

    pub fn validate(&self) -> Result<()> {
        let bad = |m: String| Err(AdlError::invalid(m));
        for p in self.points() {
            if !p.x.is_finite() || !p.y.is_finite() {
                return bad("coordinates must be finite".into());
            }
        }
        match self {
            Action::Click { count, .. } if *count == 0 || *count > MAX_CLICK_COUNT => bad(format!("click count must be 1..={MAX_CLICK_COUNT}")),
            Action::Drag { steps, .. } if *steps == 0 || *steps > MAX_DRAG_STEPS => bad(format!("drag steps must be 1..={MAX_DRAG_STEPS}")),
            Action::Scroll { steps, .. } if *steps == 0 || steps.abs() > MAX_SCROLL_STEPS => bad(format!("scroll steps must be non-zero and at most {MAX_SCROLL_STEPS}")),
            Action::Hover { ms, .. } if *ms > MAX_HOVER_MS => bad(format!("hover is limited to {MAX_HOVER_MS} ms")),
            Action::Wait { ms } if *ms > MAX_HOLD_MS => bad(format!("wait is limited to {MAX_HOLD_MS} ms")),
            Action::Type { text } if text.chars().count() > MAX_TEXT_CHARS => bad(format!("text is limited to {MAX_TEXT_CHARS} characters per batch")),
            Action::Type { text } if text.is_empty() => bad("text is empty".into()),
            Action::MoveRelative { dx, dy } if !dx.is_finite() || !dy.is_finite() => bad("deltas must be finite".into()),
            Action::Shortcut { keys } if keys.key.is_modifier() => bad("a shortcut must end in a non-modifier key".into()),
            _ => Ok(()),
        }
    }
}

/// Where keyboard input is meant to land. Typing without a target requires broad desktop control.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Target {
    /// Window the input is for. It must be focused before and during keyboard delivery.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<String>,
    /// Layout generation the agent observed. A mismatch makes the batch `target_stale`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout_generation: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InputBatch {
    pub task: String,
    #[serde(default)]
    pub target: Target,
    pub actions: Vec<Action>,
}

impl InputBatch {
    pub fn validate(&self) -> Result<()> {
        if self.actions.is_empty() {
            return Err(AdlError::invalid("batch has no actions"));
        }
        if self.actions.len() > MAX_ACTIONS_PER_BATCH {
            return Err(AdlError::invalid(format!("batch is limited to {MAX_ACTIONS_PER_BATCH} actions")));
        }
        let text: usize = self.actions.iter().map(|a| if let Action::Type { text } = a { text.chars().count() } else { 0 }).sum();
        if text > MAX_TEXT_CHARS {
            return Err(AdlError::invalid(format!("text is limited to {MAX_TEXT_CHARS} characters per batch")));
        }
        self.actions.iter().try_for_each(Action::validate)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actions_round_trip_and_validate() {
        let json = r#"{"task":"t1","target":{"window":"w1"},"actions":[
            {"type":"click","at":{"space":"display","display":"d1","x":10,"y":20}},
            {"type":"shortcut","keys":"Ctrl+S"},
            {"type":"drag","from":{"space":"desktop","x":0,"y":0},"to":{"space":"window","window":"w1","x":5,"y":5}}]}"#;
        let b: InputBatch = serde_json::from_str(json).unwrap();
        b.validate().unwrap();
        assert_eq!(b.actions[0], Action::Click { button: Button::Left, count: 1, at: Some(Point::display("d1", 10.0, 20.0)), modifiers: vec![] });
        let bad = InputBatch { task: "t".into(), target: Target::default(), actions: vec![Action::Scroll { axis: Axis::Vertical, steps: 0, at: None }] };
        assert!(bad.validate().is_err());
    }
}
