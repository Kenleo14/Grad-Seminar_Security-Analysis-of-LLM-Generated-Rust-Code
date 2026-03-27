//! # Fixed rust_binder Node Death-List Handling (CVE-2025-68260)
//!
//! This module contains the **post-fix** implementation of `Node::release`
//! for the Rust Android Binder driver.
//!
//! ## CVE-2025-68260 Summary (First Rust CVE in Linux)
//! - **Root cause**: In `Node::release`, `core::mem::take` moved the entire
//!   intrusive `death_list` to a stack temporary, the lock was dropped,
//!   and the temporary list was iterated unlocked.
//! - **Unsafe violation**: Concurrent `node_inner.death_list.remove(...)`
//!   (with SAFETY comment assuming "this list or none") raced on the
//!   embedded `prev`/`next` pointers now owned by the *stack* list.
//! - **Aliasing rules broken**: Two different `List` instances had overlapping
//!   mutable access to the same intrusive links → data race / UB.
//! - **Manifestation**: Kernel oops (DoS) via corrupted list pointers.
//! - **Fix strategy**: Pop elements *one-by-one from the original list* under
//!   the lock, process unlocked, reacquire only for the next pop.
//!   This keeps the synchronization invariant alive for the *entire*
//!   lifecycle of every list element transfer.
//!
//! The `oneway_todo` path already used this safe per-item pattern;
//! `death_list` now matches it exactly.
//!
//! ## Why this maintains Rust's aliasing & synchronization invariants
//! - The *canonical* `death_list` instance remains the sole owner of
//!   every `NodeDeath`'s links at all times.
//! - No second `List` ever exists while the lock is dropped.
//! - Every mutation of the list (pop/remove) happens while the lock
//!   is held; processing happens only after the element has been
//!   exclusively removed.
//! - No data race on `prev`/`next` fields is possible.
//!
//! Edge cases covered:
//! - Empty list → immediate return.
//! - Single element → one lock release/reacquire.
//! - Long list under contention → lock is released promptly after each pop.
//! - Nested callbacks in `set_dead` → cannot deadlock (lock not held).
//! - Concurrent registration/unregistration → fully serialized by the mutex.

#![no_std]
#![allow(unused)] // For illustration; real kernel code has full context

use kernel::prelude::*;
use kernel::sync::MutexGuard;
use kernel::linked_list::List; // kernel's intrusive List

/// Minimal illustration of the types involved (real kernel code has more fields).
pub struct NodeInner {
    pub oneway_todo: List<Arc<WorkItem>>, // simplified
    pub death_list: List<Arc<NodeDeath>>,
}

pub struct Node {
    pub owner: Arc<Process>, // owns the inner mutex
    pub inner: /* ... some guarded inner accessor ... */,
}

/// Placeholder for real kernel types.
pub struct Process {
    inner: Mutex<NodeInner>,
}
pub struct WorkItem;
pub struct NodeDeath;

impl NodeDeath {
    pub fn into_arc(self) -> Arc<Self> { todo!("real impl") }
    pub fn set_dead(self: Arc<Self>) { /* notify clients */ }
}

impl WorkItem {
    pub fn into_arc(self) -> Arc<Self> { todo!("real impl") }
    pub fn cancel(self: Arc<Self>) { /* cancel work */ }
}

impl Node {
    /// Fixed `release` – maintains lock-protected list invariant for the
    /// entire transfer lifecycle.
    pub(crate) fn release(&self) {
        let mut guard = self.owner.inner.lock();

        // First drain oneway_todo (already used the safe pattern pre-CVE)
        while let Some(work) = self.inner.access_mut(&mut guard).oneway_todo.pop_front() {
            // Release lock before potentially long-running work
            drop(guard);
            work.into_arc().cancel();
            // Reacquire for the next item
            guard = self.owner.inner.lock();
        }

        // Fixed death_list path – the actual CVE fix.
        // We pop one element at a time from the *original* list while the
        // lock is held. This guarantees:
        // 1. The intrusive links are never owned by a second List.
        // 2. No concurrent remove can touch the same prev/next pointers.
        // 3. The SAFETY contract on List::remove remains valid everywhere.
        while let Some(death) = self.inner.access_mut(&mut guard).death_list.pop_front() {
            // Element is now exclusively removed from the canonical list.
            // Lock can safely be dropped for processing.
            drop(guard);
            death.into_arc().set_dead();
            // Reacquire only for the next pop (or exit if list empty).
            guard = self.owner.inner.lock();
        }
        // Lock is dropped automatically when `guard` goes out of scope.
    }
}

// In real kernel code this would live in drivers/android/binder/node.rs
// The patch that landed in stable (3e0ae02ba831) is exactly this change
// for the death_list loop.