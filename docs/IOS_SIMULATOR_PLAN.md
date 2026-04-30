# iOS Simulator Accessibility Implementation

## Goal
Enable LLMs to "see" and interact with iOS apps running in the iOS Simulator via direct FFI to Apple's private frameworks (no idb dependency).

## Key Finding
macOS AXUIElement APIs **cannot** access iOS app accessibility trees. iOS apps in the Simulator use iOS's `UIAccessibility` framework, which is sandboxed.

Apple's private `AccessibilityPlatformTranslation` framework bridges this gap. We'll call it directly via objc2.

---

## Architecture

```
Rust (`accessibility-core`)
    ↓ objc2 FFI
AccessibilityPlatformTranslation.framework (private)
    ↓
AXPTranslator singleton ← bridgeTokenDelegate (our Rust impl)
    ↓
AXPMacPlatformElement (iOS element as macOS-like object)
    ↓
CoreSimulator.framework → SimDevice.sendAccessibilityRequestAsync
    ↓
XPC → iOS Simulator
```

---

## Token/Delegate System (for multi-simulator support)

### Why Tokens?
- `AXPTranslator` is a **singleton**
- Multiple simulators can be running simultaneously
- Tokens route requests to the correct simulator's `SimDevice`

### Protocol: `AXPTranslationTokenDelegateHelper`

```objc
@protocol AXPTranslationTokenDelegateHelper

// Called by AXPTranslator when it needs accessibility data
// Returns a block that synchronously queries CoreSimulator
- (AXPTranslationCallback)accessibilityTranslationDelegateBridgeCallbackWithToken:(NSString *)token;

// Coordinate conversion (return unchanged for our use case)
- (CGRect)accessibilityTranslationConvertPlatformFrameToSystem:(CGRect)rect withToken:(NSString *)token;

// Root parent (return nil)
- (id)accessibilityTranslationRootParentWithToken:(NSString *)token;

@end

// AXPTranslationCallback type
typedef AXPTranslatorResponse * (^AXPTranslationCallback)(AXPTranslatorRequest *request);
```

### Request Flow

```
1. Generate UUID token
2. Register token → SimDevice mapping
3. Call: translator.frontmostApplicationWithDisplayId(0, token)
       ↓
4. AXPTranslator calls: delegate.accessibilityTranslationDelegateBridgeCallbackWithToken(token)
       ↓
5. Look up SimDevice from token map
6. Call: device.sendAccessibilityRequestAsync(request, queue, handler)
       ↓
7. Block synchronously (dispatch_group_wait)
       ↓
8. Return AXPTranslatorResponse to AXPTranslator
       ↓
9. AXPTranslator returns AXPTranslationObject
       ↓
10. Call: translator.macPlatformElementFromTranslation(translation)
       ↓
11. Read properties from AXPMacPlatformElement
       ↓
12. IMPORTANT: Set element.translation.bridgeDelegateToken = token for children
       ↓
13. Unregister token when done
```

---

## Key Classes (from idb headers)

### AXPTranslator
```objc
+ (id)sharedInstance;
@property (nonatomic) __weak id<AXPTranslationTokenDelegateHelper> bridgeTokenDelegate;
- (AXPTranslationObject *)frontmostApplicationWithDisplayId:(unsigned int)displayId
                                            bridgeDelegateToken:(NSString *)token;
- (AXPTranslationObject *)objectAtPoint:(CGPoint)point
                              displayId:(unsigned int)displayId
                     bridgeDelegateToken:(NSString *)token;
- (AXPMacPlatformElement *)macPlatformElementFromTranslation:(AXPTranslationObject *)translation;
```

### AXPMacPlatformElement
```objc
@property (readonly) NSString *accessibilityLabel;
@property (readonly) NSString *accessibilityValue;
@property (readonly) NSString *accessibilityRole;
@property (readonly) NSString *accessibilityTitle;
@property (readonly) NSString *accessibilityIdentifier;
@property (readonly) CGRect accessibilityFrame;
@property (readonly) NSArray *accessibilityChildren;
@property (readonly) NSArray<NSString *> *accessibilityActionNames;
@property (readonly) BOOL accessibilityEnabled;
@property (readonly) int pid;
- (BOOL)accessibilityPerformPress;
```

### SimDevice (CoreSimulator)
```objc
- (void)sendAccessibilityRequestAsync:(AXPTranslatorRequest *)request
                       completionQueue:(dispatch_queue_t)queue
                       completionHandler:(void (^)(AXPTranslatorResponse *))handler;
```

---

## Implementation Phases

### Phase 1: Framework Loading & SimDevice Access
1. Create `crates/accessibility-core/src/platform/ios_simulator.rs`
2. Load private frameworks via dlopen:
   ```rust
   dlopen("/System/Library/PrivateFrameworks/AccessibilityPlatformTranslation.framework/...")
   dlopen("/Library/Developer/PrivateFrameworks/CoreSimulator.framework/...")
   ```
3. Get booted simulator UDID via `xcrun simctl list --json`
4. Get `SimDevice` handle (via `SimDeviceSet` or direct class lookup)

### Phase 2: Delegate Implementation
1. Define Rust struct with `declare_class!`:
   ```rust
   declare_class!(
       struct TranslationDispatcher;

       unsafe impl ClassType for TranslationDispatcher {
           type Super = NSObject;
       }

       impl TranslationDispatcher {
           // Protocol methods...
       }
   );
   ```
2. Implement `accessibilityTranslationDelegateBridgeCallbackWithToken:`:
   - Look up SimDevice from token
   - Return block that calls `sendAccessibilityRequestAsync`
   - Use dispatch_group for sync waiting
3. Implement frame conversion (return unchanged)
4. Implement root parent (return nil)
5. Register: `translator.bridgeTokenDelegate = dispatcher`

### Phase 3: Accessibility Queries
1. Generate UUID token: `NSUUID.UUID.UUIDString`
2. Register in `token_to_device: HashMap<String, SimDevice>`
3. Call `translator.frontmostApplicationWithDisplayId(0, token)`
4. Set `translation.bridgeDelegateToken = token`
5. Call `translator.macPlatformElementFromTranslation(translation)`
6. Extract properties, recursively walk children
7. **Critical**: Set token on each child's translation
8. Clean up token mapping when done

### Phase 4: Element Tree Building
1. Extract: label, role, frame, enabled, children, identifier, actionNames
2. Map iOS roles to `accesskit::Role`
3. Build `ElementTree` with sequential IDs
4. Implement filtering (depth, max_elements, interactive_only)
5. Reuse existing `ElementCache` from macOS impl

### Phase 5: Actions
1. Check `accessibilityActionNames` contains "AXPress"
2. Call `element.accessibilityPerformPress()`
3. Add coordinate-based: `translator.objectAtPoint(point, 0, token)`

### Phase 6: Multi-Simulator Support
1. Accept optional UDID parameter
2. List booted simulators: `xcrun simctl list booted --json`
3. Default to first booted if not specified
4. Thread-safe token map (Mutex)

---

## Files to Create/Modify

| File | Purpose |
|------|---------|
| `crates/accessibility-core/src/platform/ios_simulator.rs` | Main implementation |
| `crates/accessibility-core/src/platform/mod.rs` | Add `ios_simulator` module |
| `crates/accessibility-cli/examples/ios_demo.rs` | Demo example |

---

## API Design

```rust
pub struct IOSSimulatorAccessibility {
    dispatcher: Retained<TranslationDispatcher>,
    translator: *mut AnyObject,
    device: *mut AnyObject,  // SimDevice
    cache: ElementCache,
}

impl IOSSimulatorAccessibility {
    /// Create reader for specific simulator (or first booted if None)
    pub fn new(udid: Option<&str>) -> Result<Self>;

    /// Get accessibility tree from frontmost app
    pub fn get_tree(&mut self, filter: &TreeFilter) -> Result<ElementTree>;

    /// Perform action on element by ID
    pub fn perform_action(&mut self, id: ElementId, action: Action) -> Result<()>;

    /// Tap at screen coordinates
    pub fn tap(&mut self, x: f64, y: f64) -> Result<()>;

    /// Get element at point
    pub fn element_at_point(&mut self, x: f64, y: f64) -> Result<Option<Element>>;

    pub fn clear_cache(&mut self);
}
```

---

## Verification Plan

### Test 1: Framework Loading
```bash
cargo run --example ios_demo -- --test-load
```
Expected: "Frameworks loaded, translator singleton obtained"

### Test 2: Delegate Registration
```bash
cargo run --example ios_demo -- --test-delegate
```
Expected: "Delegate registered with AXPTranslator"

### Test 3: Full Tree Query (with Simulator running Settings app)
```bash
cargo run --example ios_demo
```
Expected output:
```
# iOS Simulator App: Settings (pid: 12345)

[1] Application "Settings" (500 elements, 50 interactive)
  [2] Window (480 elements, 48 interactive)
    [3] NavigationBar "Settings" (10 elements, 3 interactive)
      [4] Button "Back"
```

### Test 4: Action
```bash
cargo run --example ios_demo -- --tap-first-button
```
Expected: First button gets tapped, UI responds in simulator

### Test 5: Multi-Simulator
```bash
# Boot two simulators
cargo run --example ios_demo -- --udid <UDID_1>
cargo run --example ios_demo -- --udid <UDID_2>
```
Expected: Different trees from each simulator

### Manual Verification
1. Boot iOS Simulator with Settings app
2. Run `ios_demo` example
3. Verify output matches visible UI structure
4. Test tapping a button
5. Verify tree updates after navigation

---

## Reference: idb Source Files

Key files in the idb repository:

| Path | Contents |
|------|----------|
| `PrivateHeaders/AccessibilityPlatformTranslation/AXPTranslator.h` | Translator singleton + protocols |
| `PrivateHeaders/AccessibilityPlatformTranslation/AXPMacPlatformElement.h` | Element properties |
| `PrivateHeaders/AccessibilityPlatformTranslation/AXPTranslationObject.h` | Translation wrapper |
| `PrivateHeaders/CoreSimulator/SimDevice.h` | SimDevice.sendAccessibilityRequestAsync |
| `FBSimulatorControl/Commands/FBSimulatorAccessibilityCommands.m` | Full working implementation |
