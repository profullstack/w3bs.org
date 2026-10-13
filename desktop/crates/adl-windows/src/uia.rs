//! UI Automation tree of one window (control view), bounded in depth and node count.
//!
//! Values of password fields are never read. Handles are `<window id>/<child path>` and expire
//! with the observation like every other handle.

use crate::map;
use crate::win;
use adl_schema::*;
use windows::core::Interface;
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
use windows::Win32::UI::Accessibility::*;

pub const MAX_DEPTH: usize = 12;
pub const MAX_NODES: usize = 500;
/// Cap on a single value string (a document can be huge).
const MAX_VALUE_CHARS: usize = 4096;

pub fn automation() -> Result<IUIAutomation> {
    win::ensure_com();
    // SAFETY: COM is initialized on this thread.
    let a: IUIAutomation = unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) }.map_err(|e| AdlError::unsupported(format!("UI Automation is unavailable: {e}")))?;
    // A hung application must not hang the broker: bound UIA's cross-process calls.
    if let Ok(a2) = a.cast::<IUIAutomation2>() {
        // SAFETY: plain setters.
        unsafe {
            let _ = a2.SetConnectionTimeout(2000);
            let _ = a2.SetTransactionTimeout(2000);
        }
    }
    Ok(a)
}

struct Walk<'a> {
    walker: &'a IUIAutomationTreeWalker,
    nodes: usize,
}

fn has(el: &IUIAutomationElement, id: UIA_PATTERN_ID) -> bool {
    // SAFETY: plain COM call; a missing pattern is an error or a null interface.
    unsafe { el.GetCurrentPattern(id) }.is_ok_and(|p| !p.as_raw().is_null())
}

fn node(el: &IUIAutomationElement, handle: String) -> AccessNode {
    // SAFETY: plain COM property reads; failures fall back to defaults.
    unsafe {
        let role = el.CurrentControlType().map(|t| map::control_type_name(t.0)).unwrap_or("unknown").to_string();
        let name = el.CurrentName().map(|b| b.to_string()).unwrap_or_default();
        let bounds = el.CurrentBoundingRectangle().ok().filter(|r| r.right > r.left && r.bottom > r.top).map(|r| Rect {
            x: r.left as f64,
            y: r.top as f64,
            width: (r.right - r.left) as f64,
            height: (r.bottom - r.top) as f64,
        });
        let password = el.CurrentIsPassword().is_ok_and(|b| b.as_bool());
        let value_pattern = el.GetCurrentPattern(UIA_ValuePatternId).ok().and_then(|p| p.cast::<IUIAutomationValuePattern>().ok());
        let mut value = None;
        let mut actions = vec![];
        let mut states = vec![];
        if let Some(vp) = &value_pattern {
            if !password {
                value = vp.CurrentValue().ok().map(|b| b.to_string().chars().take(MAX_VALUE_CHARS).collect());
            }
            if !vp.CurrentIsReadOnly().is_ok_and(|b| b.as_bool()) {
                actions.push("set_value".to_string());
            }
        }
        for (id, a) in [
            (UIA_InvokePatternId, "invoke"),
            (UIA_TogglePatternId, "toggle"),
            (UIA_ExpandCollapsePatternId, "expand_collapse"),
            (UIA_SelectionItemPatternId, "select"),
            (UIA_ScrollPatternId, "scroll"),
        ] {
            if has(el, id) {
                actions.push(a.to_string());
            }
        }
        if el.CurrentIsKeyboardFocusable().is_ok_and(|b| b.as_bool()) {
            actions.push("focus".into());
        }
        if el.CurrentHasKeyboardFocus().is_ok_and(|b| b.as_bool()) {
            states.push("focused".into());
        }
        if !el.CurrentIsEnabled().map(|b| b.as_bool()).unwrap_or(true) {
            states.push("disabled".into());
        }
        if el.CurrentIsOffscreen().is_ok_and(|b| b.as_bool()) {
            states.push("offscreen".into());
        }
        if password {
            states.push("protected".into());
        }
        AccessNode { handle, role, name, value, bounds, actions, states, children: vec![] }
    }
}

impl Walk<'_> {
    fn build(&mut self, el: &IUIAutomationElement, handle: String, depth: usize) -> AccessNode {
        self.nodes += 1;
        let mut n = node(el, handle.clone());
        if depth >= MAX_DEPTH {
            return n;
        }
        // SAFETY: plain COM tree navigation.
        let mut child = unsafe { self.walker.GetFirstChildElement(el) }.ok();
        let mut i = 0;
        while let Some(c) = child {
            if self.nodes >= MAX_NODES {
                n.states.push("truncated".into());
                break;
            }
            n.children.push(self.build(&c, format!("{handle}.{i}"), depth + 1));
            i += 1;
            child = unsafe { self.walker.GetNextSiblingElement(&c) }.ok();
        }
        n
    }
}

pub fn inspect(hwnd: HWND, window_id: &str) -> Result<AccessNode> {
    let a = automation()?;
    // SAFETY: plain COM calls.
    let root = unsafe { a.ElementFromHandle(hwnd) }.map_err(|e| AdlError::stale(format!("window {window_id} is not accessible: {e}")))?;
    let walker = unsafe { a.ControlViewWalker() }.map_err(|e| AdlError::internal(format!("UI Automation walker: {e}")))?;
    let mut w = Walk { walker: &walker, nodes: 0 };
    Ok(w.build(&root, format!("{window_id}/0"), 0))
}
