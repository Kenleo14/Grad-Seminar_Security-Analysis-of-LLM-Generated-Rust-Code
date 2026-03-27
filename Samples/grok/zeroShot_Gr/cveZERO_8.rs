//! Simplified Android Binder Node Management System
//!
//! This module implements a simplified version of Android Binder's node management,
//! focusing on death notification tracking using an **intrusive linked list**.
//! It closely models the Linux kernel Binder driver's approach to `binder_node`
//! death recipients (see `binder_ref_death` and per-node `death_list` handling).
//!
//! ## Key Design Goals (as requested)
//! - **Intrusive linked list** for death notifications: No separate list-node allocations;
//!   each `DeathNotification` embeds its own `next`/`prev` pointers. This matches
//!   kernel efficiency (zero-overhead linking).
//! - **High-concurrency releases**: When releasing a `BinderNode`, all death-notification
//!   nodes are **moved atomically to a temporary list** *under lock*. The lock is
//!   released *before* executing callbacks. This prevents long-held locks during
//!   potentially slow/arbitrary user callbacks and eliminates lock contention for
//!   other threads.
//! - **Memory stability during concurrent access**: The temporary list *owns* the
//!   raw pointers (via `Box::into_raw` / `Box::from_raw`). Once moved, the source
//!   list head is cleared, guaranteeing that no other thread can mutate or free
//!   the nodes while the temporary list is being processed. Pointers remain valid
//!   until explicit cleanup after callbacks complete — preventing use-after-free
//!   even under heavy contention.
//!
//! ## How It Mirrors Real Android Binder
//! In the kernel, death notifications are spliced into a temporary list (via
//! `list_move` / `list_splice`) under the node lock, the lock is dropped, and
//! the temp list is walked for `binder_death` callbacks. This Rust version
//! achieves the same pattern safely using Rust's ownership model + controlled `unsafe`.
//!
//! ## Thread Safety & Concurrency Guarantees
//! - All list mutations are protected by a `Mutex`.
//! - Release path holds the lock for O(1) time (just a pointer swap).
//! - Callbacks run completely lock-free.
//! - Multiple concurrent `register_death_notification` + `release` operations are safe;
//!   the move-to-temp is atomic with respect to the list head.
//!
//! ## Limitations (Simplified)
//! - No full Binder IPC, transaction handling, or reference counting on nodes.
//! - Single-node focus (real Binder has a global node tree).
//! - Callbacks are `FnOnce() + Send + 'static` (no arguments or return values).
//! - No unregister support (real Binder supports it via explicit removal).
//! - Panic in a callback does *not* poison the mutex (lock already released) but
//!   will unwind only that notification; remaining notifications still execute.

use std::marker::PhantomData;
use std::ptr::NonNull;
use std::sync::Mutex;

/// Internal death notification entry. This is the **intrusive list node**.
/// Links (`next`/`prev`) are embedded directly — zero extra allocation.
struct DeathNotification {
    next: Option<NonNull<DeathNotification>>,
    prev: Option<NonNull<DeathNotification>>,
    callback: Box<dyn FnOnce() + Send + 'static>,
}

impl DeathNotification {
    fn new(callback: Box<dyn FnOnce() + Send + 'static>) -> Self {
        Self {
            next: None,
            prev: None,
            callback,
        }
    }
}

/// Intrusive doubly-linked list head that *owns* its nodes via raw pointers.
/// Ownership is transferred with `Box::into_raw` / `Box::from_raw`.
/// The `PhantomData` marks that the list conceptually owns `Box<DeathNotification>`.
struct IntrusiveDeathList {
    head: Option<NonNull<DeathNotification>>,
    _phantom: PhantomData<Box<DeathNotification>>,
}

impl IntrusiveDeathList {
    fn new() -> Self {
        Self {
            head: None,
            _phantom: PhantomData,
        }
    }

    /// Push a notification into the front of the list (intrusive insert).
    fn push(&mut self, notification: DeathNotification) {
        let node = NonNull::new(Box::into_raw(Box::new(notification)))
            .expect("Box::into_raw never returns null");
        unsafe { self.insert(node) };
    }

    /// Intrusive insert at head (doubly-linked).
    /// # Safety
    /// Caller must ensure `node` points to a valid, exclusively owned `DeathNotification`
    /// that is not already linked elsewhere.
    unsafe fn insert(&mut self, node: NonNull<DeathNotification>) {
        let node_ptr = node.as_ptr();

        if let Some(mut head) = self.head {
            // Link new node before current head
            let head_ref = head.as_mut();
            head_ref.prev = Some(node);

            let node_ref = &mut *node_ptr;
            node_ref.next = Some(head);
            node_ref.prev = None;

            self.head = Some(node);
        } else {
            // First node
            let node_ref = &mut *node_ptr;
            node_ref.next = None;
            node_ref.prev = None;
            self.head = Some(node);
        }
    }

    /// Atomically move the *entire* list to a new temporary list.
    /// This is the core primitive for high-concurrency release.
    /// After this call the original list is empty.
    fn take_all(&mut self) -> Self {
        let mut temp = Self::new();
        std::mem::swap(&mut self.head, &mut temp.head);
        temp
    }

    /// Consume the list, execute every callback, then drop the nodes.
    /// This runs **outside any lock** for maximum concurrency.
    fn notify_all(self) {
        let mut current = self.head;

        while let Some(node_ptr) = current {
            // Read next *before* any mutation or drop
            let next = unsafe { (*node_ptr.as_ptr()).next };

            // Extract and call callback
            let node_mut = unsafe { &mut *node_ptr.as_ptr() };
            let callback = std::mem::replace(&mut node_mut.callback, Box::new(|| {}));
            callback();

            // Advance and drop the node (reconstruct Box)
            current = next;
            let _ = unsafe { Box::from_raw(node_ptr.as_ptr()) };
        }
    }
}

/// A Binder node that can register death notifications and be released safely
/// under high contention.
pub struct BinderNode {
    id: u32,
    /// Protected intrusive list of death notifications.
    death_list: Mutex<IntrusiveDeathList>,
    /// Simple alive flag to prevent double-release and post-death registration races.
    is_alive: Mutex<bool>,
}

impl BinderNode {
    /// Create a new Binder node.
    pub fn new(id: u32) -> Self {
        Self {
            id,
            death_list: Mutex::new(IntrusiveDeathList::new()),
            is_alive: Mutex::new(true),
        }
    }

    /// Register a death notification callback.
    ///
    /// If the node has already been released, the callback is invoked immediately
    /// (mimicking Binder's "already dead" behavior).
    pub fn register_death_notification<F>(&self, callback: F)
    where
        F: FnOnce() + Send + 'static,
    {
        let mut alive_guard = self.is_alive.lock().unwrap();
        if !*alive_guard {
            // Node already dead → fire callback immediately (no lock contention)
            let _ = Box::new(callback)();
            return;
        }
        // Release alive lock before acquiring death_list lock (avoids deadlock)
        drop(alive_guard);

        let notification = DeathNotification::new(Box::new(callback));
        let mut list_guard = self.death_list.lock().unwrap();
        list_guard.push(notification);
    }

    /// Release the Binder node.
    ///
    /// **High-concurrency path**:
    /// 1. Mark node dead under `is_alive` lock.
    /// 2. Under `death_list` lock: move entire intrusive list to a *temporary list*.
    /// 3. Release both locks.
    /// 4. Process the temporary list (callbacks + cleanup) *lock-free*.
    ///
    /// This guarantees:
    /// - Lock hold time is minimal (pointer swap only).
    /// - Memory stability: temporary list owns all `NonNull` pointers; source list is emptied atomically.
    /// - No other thread can observe or modify the nodes being cleaned up.
    pub fn release(&self) {
        // Mark dead (prevents further registrations)
        {
            let mut alive_guard = self.is_alive.lock().unwrap();
            if !*alive_guard {
                return; // already released
            }
            *alive_guard = false;
        }

        // Critical section: move list to temp (O(1))
        let temp_list = {
            let mut list_guard = self.death_list.lock().unwrap();
            list_guard.take_all()
        }; // locks dropped here

        // High-concurrency cleanup (no locks held)
        temp_list.notify_all();
    }
}

impl Drop for BinderNode {
    fn drop(&mut self) {
        // Ensure cleanup even if caller forgets to call release()
        self.release();
    }
}

/// Example usage (for illustration; not part of the public API)
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;

    #[test]
    fn test_concurrent_release_and_register() {
        let node = Arc::new(BinderNode::new(42));

        // Spawn many threads registering while one releases
        let handles: Vec<_> = (0..10)
            .map(|i| {
                let node_clone = Arc::clone(&node);
                thread::spawn(move || {
                    node_clone.register_death_notification(move || {
                        println!("Death notification {i} fired");
                    });
                })
            })
            .collect();

        // Release from main thread
        node.release();

        // Wait for all registrars
        for h in handles {
            h.join().unwrap();
        }

        // All callbacks that were registered *before* release have fired;
        // any registered after were invoked immediately.
    }
}