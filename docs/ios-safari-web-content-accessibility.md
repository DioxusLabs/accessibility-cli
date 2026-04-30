# iOS Safari Web Content Accessibility - Research Resources

## Problem

iOS Safari web content elements are isolated in a separate WebContent process and don't expose a traversable accessibility hierarchy. Standard `AXPTranslator` APIs only return toolbar elements.

---

## Key Resources

### idb (Facebook iOS Development Bridge)

The most complete solution for iOS simulator accessibility.

- **Repository**: https://github.com/facebook/idb
- **Installation**: `brew install idb-companion`
- **Accessibility docs**: https://fbidb.io/docs/accessibility/

Key source files:
- **FBSimulatorBridge.h/.m**: https://github.com/facebook/idb/blob/main/FBSimulatorControl/Management/FBSimulatorBridge.h
  - Shows how to connect to SimulatorBridge via Distributed Objects
  - `accessibilityElementsWithDisplayId:` returns all elements as JSON

- **Private Headers**: https://github.com/facebook/idb/tree/main/PrivateHeaders
  - `AccessibilityPlatformTranslation/` - AXPTranslator, AXPMacPlatformElement headers
  - `SimulatorBridge/` - SimulatorBridge protocol with accessibility methods
  - `AXRuntime/` - AXTraits definitions

### iOS Runtime Headers

Dumped headers from iOS frameworks:

- **iOS 10 Headers**: https://github.com/JaviSoto/iOS10-Runtime-Headers
  - `AXRuntime/AXRemoteElement.h` - Remote accessibility element class

- **iOS Headers Collection**: https://github.com/nst/iOS-Runtime-Headers/tree/master/PrivateFrameworks

- **iOS Private Headers**: https://github.com/ichitaso/iOS-iphoneheaders
  - Includes AccessibilityUIServer headers

### WebKit Accessibility Source

How WebKit implements accessibility internally:

- **iOS Wrapper**: https://github.com/WebKit/webkit/blob/main/Source/WebCore/accessibility/ios/WebAccessibilityObjectWrapperIOS.mm
  - `accessibilityElements` - returns all unignored children
  - `accessibilityElementAtIndex:` - child access

- **macOS Wrapper**: https://github.com/WebKit/webkit/blob/main/Source/WebCore/accessibility/mac/WebAccessibilityObjectWrapperMac.mm

- **Accessibility Performance**: https://trac.webkit.org/wiki/WebKitAccessibilityPerformance
  - Explains AX tree building, IsolatedTree mode

### Apple Entitlements Database

Shows what permissions accessibility services have:

- **AccessibilityUIServer entitlements**: https://newosxbook.com/ent.jl?osVer=iOS16&exec=System/Library/CoreServices/AccessibilityUIServer.app/AccessibilityUIServer

### Related Tools

- **AXe CLI**: https://github.com/cameroncooke/AXe
  - CLI tool using idb's frameworks for accessibility inspection

- **ios-webkit-debug-proxy**: https://github.com/nicknisi/ios-webkit-debug-proxy
  - Chrome DevTools proxy for iOS WebViews (different approach)

---

## Local Framework Paths

On macOS with Xcode installed:

```
# AccessibilityPlatformTranslation (AXPTranslator, AXPMacPlatformElement)
/System/Library/PrivateFrameworks/AccessibilityPlatformTranslation.framework/

# CoreSimulator (SimDevice, accessibility XPC)
/Library/Developer/PrivateFrameworks/CoreSimulator.framework/

# CoreSimulatorBridge binary
/Library/Developer/PrivateFrameworks/CoreSimulator.framework/Versions/A/Resources/Platforms/iphoneos/usr/libexec/CoreSimulatorBridge

# CoreSimulatorBridge launchd plist (shows registered services)
/Library/Developer/PrivateFrameworks/CoreSimulator.framework/Versions/A/Resources/Platforms/iphoneos/Library/LaunchDaemons/com.apple.CoreSimulator.bridge.plist
```

---

## Key Technical Findings

### SimulatorBridge Port Registration

idb spawns `SimulatorBridge` from Simulator.app bundle with a port name argument:
```
{Simulator.app}/Contents/Resources/Platforms/iphoneos/usr/libexec/SimulatorBridge com.apple.iphonesimulator.bridge.FBSimulatorControl
```

The process outputs "READY" when initialized, then the port can be looked up:
```objc
mach_port_t port = [device lookup:@"com.apple.iphonesimulator.bridge.FBSimulatorControl" error:&error];
```

### CoreSimulatorBridge Services

From the launchd plist, CoreSimulatorBridge registers these mach services:
- `com.apple.CoreSimulator.accessibility`
- `com.apple.CoreSimulator.bridge`
- `com.apple.CoreSimulator.host_support`
- `com.apple.CoreSimulator.pasteboard_support`

These use XPC, not Distributed Objects, and have different APIs than idb's SimulatorBridge.

### Web Content Process Isolation

- WebContent process: `com.apple.WebKit.WebContent`
- Elements have `AXParent: null`
- `accessibilityChildren` returns empty
- Only discoverable via `objectAtPoint:` hit testing

---

## Relevant WebKit Bugs / Discussions

- **Bug 203798**: AX: WKWebView does not shift Accessibility Focus for Catalyst
  https://bugs.webkit.org/show_bug.cgi?id=203798

- **Bug 211238**: [iOS] Every running WebContent process should be granted access to frontboard services when Accessibility is enabled
  https://bugs.webkit.org/show_bug.cgi?id=211238

- **Mozilla Platform Tilt Issue**: Accessibility APIs on iOS
  https://github.com/mozilla/platform-tilt/issues/4
  - Documents undocumented iOS accessibility APIs
  - Notes closed-source WebKit accessibility bundle

---

## Commands for Debugging

```bash
# List accessibility services in simulator
xcrun simctl spawn booted launchctl list | grep -i accessibility

# Check available mach services
xcrun simctl spawn booted launchctl print system | grep -i bridge

# Use idb to get all elements (if installed)
idb ui describe-all --udid <UDID>

# Analyze framework exports
dyld_info -exports /System/Library/PrivateFrameworks/AccessibilityPlatformTranslation.framework/AccessibilityPlatformTranslation
```
