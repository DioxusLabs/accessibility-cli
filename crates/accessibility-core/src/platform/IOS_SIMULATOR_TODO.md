# iOS Simulator Implementation: Missing Features from idb

This document tracks features from Meta's [idb](https://github.com/facebook/idb) that could be added to our iOS Simulator implementation.

## Current Implementation Status

**Working:**
- Accessibility tree reading via `AccessibilityPlatformTranslation` framework
- Token-based delegation for multi-simulator support
- HID injection via Indigo protocol (tap, swipe, buttons, keyboard)
- Zero-frame remediation (restart CoreSimulatorBridge)
- Element caching and action support

---

## Accessibility Features

### High Priority

| Feature | idb Reference | Description |
|---------|---------------|-------------|
| **accessibilityIdentifier** | `FBAXKeysUniqueID` | Developer-assigned stable ID for automation. Apps set these via `accessibilityIdentifier` property for testing. Critical for reliable element identification. |
| **Tap with label verification** | `accessibilityPerformTapOnElementAtPoint:expectedLabel:` | Verify element's label matches expected value before tapping. Prevents clicking wrong element if UI changed. |
| **Batch attribute fetching** | `accessibilityMultipleAttributes:` | Fetch multiple attributes in one XPC call. Significantly improves performance for large trees. |

### Medium Priority

| Feature | idb Reference | Description |
|---------|---------------|-------------|
| **Key filtering** | `keys` parameter | Filter which properties to return (e.g., only label+frame). Reduces data transfer for large trees. |
| **Flat format output** | `nestedFormat:NO` | Return elements as flat list instead of tree. Useful for searching/filtering. |
| **Custom actions** | `accessibilityCustomActions` | App-defined custom actions beyond standard AX actions (e.g., "Delete", "Archive"). |
| **Role description** | `accessibilityRoleDescription` | Human-readable role description (e.g., "button" vs "AXButton"). |
| **Subrole** | `accessibilitySubrole` | More specific role classification (e.g., "AXCloseButton" subrole of button). |

### Low Priority

| Feature | idb Reference | Description |
|---------|---------------|-------------|
| **Help text** | `accessibilityHelp` | Help/tooltip text for element. |
| **Content required** | `accessibilityRequired` | Whether form field is required. |

---

## HID Features

### High Priority

| Feature | idb Reference | Description |
|---------|---------------|-------------|
| **Long press** | `tapAtX:y:duration:` | Tap with configurable hold duration. Essential for context menus, drag operations. |
| **Key press sequence** | `shortKeyPressSequence:` | Type a sequence of key codes. Essential for text input via hardware keyboard. |
| **Inertial scroll fix** | Swipe implementation | Add extra touch-down at end of swipe to prevent momentum scrolling on ARM simulators. |

### Medium Priority

| Feature | idb Reference | Description |
|---------|---------------|-------------|
| **Composite events** | `FBSimulatorHIDEvent_Composite` | Chain multiple HID events into single operation with proper sequencing. |
| **Event delays** | `FBSimulatorHIDEventDelay` | Insert configurable delays between events in sequences. |
| **Swipe with delta** | `delta` parameter | Configure pixel spacing between touch points during swipe (default 10px). |

### Low Priority

| Feature | idb Reference | Description |
|---------|---------------|-------------|
| **Named key constants** | Various | Named constants for common key codes (Return=0x24, Delete=0x33, etc.). |
| **HID event logging** | `FBSIMULATORCONTROL_LOG_HID_DETAILS` | Environment variable to enable detailed HID event logging. |

---

## Architecture Improvements

| Feature | Description |
|---------|-------------|
| **Async API** | idb uses `FBFuture` for all operations. Consider adding async versions of methods. |
| **Error recovery** | More sophisticated error recovery beyond zero-frame remediation. |
| **Connection pooling** | Reuse HID client connections across operations. |

---

## Implementation Notes

### accessibilityIdentifier

```rust
// Add to Element struct
pub struct Element {
    // ... existing fields ...
    pub identifier: Option<String>,  // accessibilityIdentifier
}

// Add getter
unsafe fn get_element_identifier(&self, element: *mut AnyObject) -> Option<String> {
    let identifier: *mut AnyObject = msg_send![element, accessibilityIdentifier];
    self.nsstring_to_string(identifier)
}
```

### Long Press

```rust
/// Tap with configurable hold duration.
pub fn hid_long_press(&mut self, x: f64, y: f64, duration_ms: u64) -> Result<()> {
    let hid = self.get_hid()?;
    hid.send_touch(x, y, ButtonDirection::Down)?;
    std::thread::sleep(std::time::Duration::from_millis(duration_ms));
    hid.send_touch(x, y, ButtonDirection::Up)?;
    Ok(())
}
```

### Inertial Scroll Fix

```rust
// In swipe(), add extra touch-down at end before touch-up:
fn swipe(&self, start: (f64, f64), end: (f64, f64), duration_ms: u64) -> Result<()> {
    // ... existing swipe logic ...

    // Extra touch-down to stop inertial scrolling (ARM simulator fix)
    self.send_touch(end_x_ratio, end_y_ratio, ButtonDirection::Down)?;
    std::thread::sleep(step_delay);

    // Touch up
    self.send_touch(end_x_ratio, end_y_ratio, ButtonDirection::Up)?;
    Ok(())
}
```

### Key Code Constants

```rust
/// Common key codes from HIToolbox/Events.h
pub mod KeyCode {
    pub const A: u32 = 0x00;
    pub const S: u32 = 0x01;
    pub const D: u32 = 0x02;
    // ... letters ...
    pub const RETURN: u32 = 0x24;
    pub const TAB: u32 = 0x30;
    pub const SPACE: u32 = 0x31;
    pub const DELETE: u32 = 0x33;
    pub const ESCAPE: u32 = 0x35;
    pub const LEFT_ARROW: u32 = 0x7B;
    pub const RIGHT_ARROW: u32 = 0x7C;
    pub const DOWN_ARROW: u32 = 0x7D;
    pub const UP_ARROW: u32 = 0x7E;
}
```

---

## References

- idb source: `/Users/jonathankelley/Development/Tinkering/rust-computer-use/vendor/idb/`
- Key files:
  - `FBSimulatorControl/Commands/FBSimulatorAccessibilityCommands.m`
  - `FBSimulatorControl/HID/FBSimulatorHIDEvent.m`
  - `FBSimulatorControl/HID/FBSimulatorIndigoHID.m`
  - `PrivateHeaders/AccessibilityPlatformTranslation/AXPMacPlatformElement.h`
