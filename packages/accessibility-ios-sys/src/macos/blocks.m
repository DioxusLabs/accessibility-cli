// Signature-bearing Objective-C blocks for CoreSimulator's remote proxies.
//
// CoreSimulator vends its IO descriptors as ROCKRemoteProxy objects, and
// ROCKit marshals block arguments across that boundary by reading the block's
// Objective-C type encoding out of its descriptor. That requires the
// BLOCK_HAS_SIGNATURE flag, which the `block2` crate does not currently emit
// (see the TODO in block2's global.rs). Clang always emits it, so the blocks
// handed to SimulatorKit are created here instead of in Rust.
//
// The block owns its context. SimulatorKit retains a registered block and
// keeps delivering queued invocations for a moment after
// unregisterScreenCallbacksWithUUID: returns, so the context can only be
// released once the last holder of the block lets go. The block captures an
// object whose dealloc runs the dispose callback, which the block runtime
// releases together with the block.

#import <Block.h>
#import <Foundation/Foundation.h>
#include <stddef.h>

typedef void (*accessibility_void_callback)(void *context);

@interface AccessibilityBlockContext : NSObject
@property(nonatomic, assign) accessibility_void_callback dispose;
@property(nonatomic, assign) void *context;
@end

@implementation AccessibilityBlockContext
- (void)dealloc {
    if (_dispose != NULL) {
        _dispose(_context);
    }
    [super dealloc];
}
@end

// Create a heap block wrapping `callback(context)`.
//
// The returned block is owned by the caller and must be handed to
// accessibility_release_block exactly once. `dispose(context)` runs when the
// block itself is freed, which may be later than that release if
// SimulatorKit still holds the block.
void *accessibility_make_void_block(accessibility_void_callback callback,
                                    accessibility_void_callback dispose,
                                    void *context) {
    AccessibilityBlockContext *holder = [[AccessibilityBlockContext alloc] init];
    holder.dispose = dispose;
    holder.context = context;
    void (^block)(void) = ^{
        callback(holder.context);
    };
    void *copied = (void *)Block_copy(block);
    [holder release];
    return copied;
}

void accessibility_release_block(void *block) {
    if (block != NULL) {
        Block_release(block);
    }
}
