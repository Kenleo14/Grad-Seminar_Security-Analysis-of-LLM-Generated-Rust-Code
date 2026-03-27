//! Simplified Android Binder Node Management System
//!
//! This module provides a thread-safe implementation of a Binder-like node
//! management system, modeled after key aspects of Android's Binder IPC
//! driver (drivers/android/binder.c and related structures). In the real
//! Android kernel, `binder_node` structures maintain intrusive linked lists
//! (`struct list_head`) of death notifications (`binder_death`). These lists
//! enable efficient registration, unregistration (in full implementations),
//! and notification when a remote object dies.
//!
//! **Key design choices mirroring Binder:**
//! - **Intrusive linked list** for death notifications: The link (`next` pointer)
//!   is embedded directly inside `DeathNotification`. This avoids extra heap
//!   allocations per list entry (classic intrusive-list benefit) and matches
//!   the kernel's `list_head` pattern.
//! - **High-concurrency release handling**: When a node is released (refcount
//!   drops to zero or explicit `release()` is called), the entire death list
//!   is moved to a temporary list *under the lock* via `take_for_cleanup()`.
//!   The lock is dropped *immediately*, and notifications are processed outside
//!   the critical section. This is the exact technique used in Binder's
//!   `binder_node_release()` / death-list splicing (`list_splice_init` +
//!   `list_for_each_entry_safe`) to avoid:
//!     - Deadlocks (callbacks may acquire other locks).
//!     - Long lock hold times under contention.
//!     - Memory instability / use-after-free if another thread drops the node
//!       while the list is being walked.
//! - **Memory stability guarantee**: By transferring ownership of each
//!   `DeathNotification` (via `Box::into_raw` / `Box::from_raw`) to the
//!   temporary processing path *before* the lock is released, the pointed-to
//!   memory remains valid even if the `BinderNode` itself is concurrently
//!   dropped or reused. The intrusive pointers are never dereferenced after
//!   the owning `Box` is dropped.
//!
//! **Thread-safety model**: A single `Mutex` protects the node's internal
//! state. All list mutations are performed while the lock is held. The
//! intrusive list itself is *not* lock-free; the temporary-list move provides
//! the concurrency safety required by the problem statement.
//!
//! **Simplifications** (relative to full Binder):
//! - Singly-linked intrusive list (sufficient for release-only semantics;
//!   doubly-linked would be added if unregistration were needed).
//! - No global node registry or reference counting (user can wrap `BinderNode`
//!   in `Arc` if shared ownership is required).
//! - Death notifications are fire-and-forget (`FnOnce`); no cookie or
//!   user-space binder_death_release callback.
//! - No error handling for OOM (real kernel would use `GFP_KERNEL`).
//! - Release is idempotent and can be called explicitly for testing.
//!
//! **Usage example** (outside this module):
//! ```rust
//! let node = BinderNode::new();
//! node.register_death_notification(|| println!("Node died!"));
//! // ... concurrent access from many threads ...
//! node.release(); // triggers all pending notifications safely
//! ```
//!
//! **Edge cases & considerations covered**:
//! - Multiple concurrent `register_death_notification` + `release` calls.
//! - Release called more than once (idempotent).
//! - Callbacks that themselves acquire locks or perform heavy work.
//! - Node dropped while notifications are still being processed.
//! - Empty death list on release.
//! - High-contention scenarios (lock held only for O(1) splice).

use std::ptr::NonNull;
use std::sync::Mutex;

/// Internal death notification. The `next` pointer makes the list intrusive.
struct DeathNotification {
    /// The one-time callback to execute when the node dies.
    callback: Option<Box<dyn FnOnce() + Send + 'static>>,
    /// Intrusive link to the next notification (None terminates the list).
    next: Option<NonNull<DeathNotification>>,
}

/// Intrusive singly-linked list of death notifications.
/// All operations that mutate the list are performed under the node's Mutex.
struct DeathList {
    head: Option<NonNull<DeathNotification>>,
}

impl DeathList {
    fn new() -> Self {
        Self { head: None }
    }

    /// Registers a new death notification.
    /// The `Box` is immediately leaked into the intrusive list via `into_raw`.
    fn register(&mut self, callback: Box<dyn FnOnce() + Send + 'static>) {
        let death = Box::new(DeathNotification {
            callback: Some(callback),
            next: None,
        });
        let ptr = NonNull::new(Box::into_raw(death)).expect("Box::into_raw never returns null");

        // Insert at head (O(1) intrusive prepend).
        unsafe {
            (*ptr.as_ptr()).next = self.head;
            self.head = Some(ptr);
        }
    }

    /// Atomically moves the entire death list to a temporary list for cleanup.
    /// Returns the old head pointer (or None if empty). After this call the
    /// node's list is empty.
    fn take_for_cleanup(&mut self) -> Option<NonNull<DeathNotification>> {
        self.head.take()
    }
}

/// Private inner state protected by the Mutex.
struct BinderNodeInner {
    death_list: DeathList,
    /// Prevents duplicate notifications and new registrations after death.
    released: bool,
}

impl BinderNodeInner {
    fn new() -> Self {
        Self {
            death_list: DeathList::new(),
            released: false,
        }
    }
}

/// Public API: a simplified Binder node that can register death notifications
/// and be released under high concurrency while preserving memory stability.
pub struct BinderNode {
    inner: Mutex<BinderNodeInner>,
}

impl BinderNode {
    /// Creates a new Binder node with an empty death-notification list.
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(BinderNodeInner::new()),
        }
    }

    /// Registers a death notification callback.
    /// The callback will be executed exactly once when `release()` is called,
    /// unless the node is already released (in which case the callback is
    /// dropped immediately).
    ///
    /// This method is safe to call concurrently from any number of threads.
    pub fn register_death_notification<F>(&self, callback: F)
    where
        F: FnOnce() + Send + 'static,
    {
        let callback_box: Box<dyn FnOnce() + Send + 'static> = Box::new(callback);

        let mut guard = self.inner.lock().unwrap();
        if guard.released {
            // Node already dead: drop the callback immediately (no-op semantics).
            // This matches Binder's behavior where registrations after death
            // are ignored.
            return;
        }
        guard.death_list.register(callback_box);
    }

    /// Releases the node, triggering all pending death notifications.
    ///
    /// **High-concurrency safety**:
    /// 1. Under the lock, the entire intrusive list is moved to a temporary
    ///    head pointer (O(1) splice).
    /// 2. The lock is released *before* any callbacks run.
    /// 3. Callbacks execute on the temporary list using `Box::from_raw` to
    ///    reclaim ownership, guaranteeing the memory remains valid even if
    ///    another thread drops this `BinderNode` concurrently.
    ///
    /// This is the core technique requested by the problem statement.
    pub fn release(&self) {
        let temp_head = {
            let mut guard = self.inner.lock().unwrap();
            if guard.released {
                return; // Idempotent.
            }
            guard.released = true;
            guard.death_list.take_for_cleanup()
        }; // MutexGuard dropped here → lock released

        Self::process_death_notifications(temp_head);
    }

    /// Processes the temporary list of death notifications *outside* any lock.
    /// This function walks the intrusive chain, reclaims each `Box`, runs the
    /// callback, and drops the notification.
    ///
    /// **Memory-stability reasoning**:
    /// - We read the `next` pointer *before* calling `Box::from_raw`.
    /// - `Box::from_raw` gives us exclusive ownership of the memory.
    /// - The intrusive pointer is never dereferenced after the `Box` is dropped.
    /// - No other thread can reach these nodes because the original list head
    ///   was nulled out under the lock.
    fn process_death_notifications(mut current: Option<NonNull<DeathNotification>>) {
        while let Some(ptr) = current {
            // Read the next link *before* taking ownership.
            let next = unsafe { (*ptr.as_ptr()).next };

            // Reclaim ownership of this intrusive node.
            let mut death = unsafe { Box::from_raw(ptr.as_ptr()) };

            // Execute the callback exactly once.
            if let Some(callback) = death.callback.take() {
                callback();
            }

            // The `Box` drops here, freeing the memory for this notification.
            // The next iteration uses the previously-read pointer, which is
            // still valid because we have not yet dropped the previous node
            // when we read the link.

            current = next;
        }
    }
}

impl Drop for BinderNode {
    /// If the node is dropped without an explicit `release()` call, we still
    /// notify any remaining death listeners. This matches Binder's refcount
    /// drop path.
    fn drop(&mut self) {
        // We cannot call `self.release()` directly because `&self` is not
        // available in Drop. Instead we duplicate the critical section logic
        // (acceptable for a simplified implementation).
        let temp_head = {
            let mut guard = match self.inner.lock() {
                Ok(g) => g,
                Err(_) => return, // Poisoned mutex – treat as already released.
            };
            if guard.released {
                return;
            }
            guard.released = true;
            guard.death_list.take_for_cleanup()
        };

        Self::process_death_notifications(temp_head);
    }
}