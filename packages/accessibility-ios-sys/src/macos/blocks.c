// Signature-bearing Objective-C blocks for CoreSimulator's remote proxies.
//
// CoreSimulator vends its IO descriptors as ROCKRemoteProxy objects, and
// ROCKit marshals block arguments across that boundary by reading the block's
// Objective-C type encoding out of its descriptor. That requires the
// BLOCK_HAS_SIGNATURE flag, which the `block2` crate does not currently emit
// (see the TODO in block2's global.rs). Clang always emits it, so the blocks
// handed to SimulatorKit are created here instead of in Rust.

#include <Block.h>
#include <stddef.h>

typedef void (*accessibility_void_callback)(void *context);

// Create a heap block wrapping `callback(context)`.
//
// The returned block is owned by the caller and must be handed to
// accessibility_release_block exactly once. `context` is not managed here; the
// Rust side owns it and must outlive the block.
void *accessibility_make_void_block(accessibility_void_callback callback, void *context) {
    void (^block)(void) = ^{
        callback(context);
    };
    return (void *)Block_copy(block);
}

void accessibility_release_block(void *block) {
    if (block != NULL) {
        Block_release(block);
    }
}
