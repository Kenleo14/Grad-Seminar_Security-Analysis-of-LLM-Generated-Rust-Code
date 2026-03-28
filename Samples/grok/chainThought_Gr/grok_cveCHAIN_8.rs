//! Fixed Simplified Android Binder Node Management System
//!
//! This module implements a simplified Binder node with intrusive death notifications.
//! It incorporates the official fix for CVE-2025-68260 (the first Rust CVE in the Linux kernel).
//!
//! ## Analysis of the Original Vulnerability (CVE-2025-68260)
//! In the vulnerable `Node::release` (and the previous version of this module):
//! 1. Acquire lock.
//! 2. Move entire intrusive list to a temporary stack list (`mem::take` / `take_all`).
//! 3. Drop the lock.
//! 4. Iterate the temporary list.
//!
//! Concurrently, `unsafe { death_list.remove(...) }` (used for unregister) mutated the same
//! `prev`/`next` pointers. After the move-to-stack + lock drop, two mutable aliases existed
//! to the intrusive fields, violating Rust’s aliasing rules (Stacked Borrows / Tree Borrows).
//! Result: linked-list corruption and kernel panic.
//!
//! ## How the Fix Maintains the Synchronization Invariant
//! - **Incremental pop + re-lock**: Every node is *fully detached* from the list *while the lock is held*.
//! - The “list transfer” (pop_front) and any potential `remove` are serialized for the entire
//!   lifetime of each notification.
//! - Once popped, the node is isolated; no other thread can alias its intrusive pointers.
//! - Callbacks still execute completely lock-free, preserving high-concurrency semantics.
//! - The unsafe removal path’s SAFETY comment remains valid.
//!
//! This pattern eliminates the race window while keeping the design faithful to real Binder
//! behavior and Rust’s ownership model.

use std::marker::PhantomData;
use std::ptr::NonNull;
use std::sync::Mutex;

/// Intrusive death notification entry (zero-allocation linking).
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

/// Intrusive doubly-linked list using raw pointers for kernel-style efficiency.
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

    /// Push a notification to the front (intrusive insert).
    fn push(&mut self, notification: DeathNotification) {
        let node = NonNull::new(Box::into_raw(Box::new(notification)))
            .expect("Box::into_raw never returns null");
        unsafe { self.insert(node) };
    }

    /// # Safety
    /// `node` must be a valid, exclusively owned `DeathNotification` not already linked.
    unsafe fn insert(&mut self, node: NonNull<DeathNotification>) {
        let node_ptr = node.as_ptr();

        if let Some(mut head) = self.head {
            let head_ref = head.as_mut();
            head_ref.prev = Some(node);

            let node_ref = &mut *node_ptr;
            node_ref.next = Some(head);
            node_ref.prev = None;

            self.head = Some(node);
        } else {
            let node_ref = &mut *node_ptr;
            node_ref.next = None;
            node_ref.prev = None;
            self.head = Some(node);
        }
    }

    /// Pop the front notification, fully detaching it under the lock.
    /// Returns the callback directly (node is consumed and dropped).
    fn pop_front(&mut self) -> Option<Box<dyn FnOnce() + Send + 'static>> {
        let head = self.head?;
        let node_ptr = head.as_ptr();

        // Read next *before* mutating the list head
        let next = unsafe { (*node_ptr).next };

        // Fix successor's prev pointer
        if let Some(mut next_nn) = next {
            unsafe { next_nn.as_mut() }.prev = None;
        }

        // Update head (list no longer contains this node)
        self.head = next;

        // Clear the detached node's links (defensive)
        let node_ref = unsafe { &mut *node_ptr };
        node_ref.next = None;
        node_ref.prev = None;

        // Reconstruct Box and extract callback
        let node = unsafe { Box::from_raw(node_ptr) };
        let callback = node.callback;
        Some(callback)
    }
}

/// Binder node with safe death notification handling.
pub struct BinderNode {
    id: u32,
    death_list: Mutex<IntrusiveDeathList>,
    is_alive: Mutex<bool>,
}

impl BinderNode {
    pub fn new(id: u32) -> Self {
        Self {
            id,
            death_list: Mutex::new(IntrusiveDeathList::new()),
            is_alive: Mutex::new(true),
        }
    }

    /// Register a death notification.
    /// If the node is already dead, the callback fires immediately.
    pub fn register_death_notification<F>(&self, callback: F)
    where
        F: FnOnce() + Send + 'static,
    {
        let mut alive_guard = self.is_alive.lock().unwrap();
        if !*alive_guard {
            drop(alive_guard);
            let _ = Box::new(callback)();
            return;
        }
        drop(alive_guard);

        let notification = DeathNotification::new(Box::new(callback));
        let mut list_guard = self.death_list.lock().unwrap();
        list_guard.push(notification);
    }

    /// Release the node (fixed CVE-2025-68260 path).
    ///
    /// Incremental pop ensures the synchronization invariant is maintained
    /// for the *entire* lifecycle of each list transfer:
    /// - Pop happens entirely under the lock → node is fully removed.
    /// - Lock is dropped *only* after detachment.
    /// - Callback runs lock-free.
    /// - Re-acquire for the next node.
    pub fn release(&self) {
        // Mark dead
        {
            let mut alive_guard = self.is_alive.lock().unwrap();
            if !*alive_guard {
                return;
            }
            *alive_guard = false;
        }

        // Incremental transfer under lock discipline
        let mut list_guard = self.death_list.lock().unwrap();
        loop {
            let Some(callback) = list_guard.pop_front() else {
                break;
            };

            // Node is now completely detached; invariant holds.
            // Safe to drop lock before executing arbitrary callback.
            drop(list_guard);

            // High-concurrency callback (no lock held)
            callback();

            // Re-acquire for next notification
            list_guard = self.death_list.lock().unwrap();
        }
    }
}

impl Drop for BinderNode {
    fn drop(&mut self) {
        self.release();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;

    #[test]
    fn test_concurrent_release_and_register() {
        let node = Arc::new(BinderNode::new(42));

        let handles: Vec<_> = (0..20)
            .map(|i| {
                let node_clone = Arc::clone(&node);
                thread::spawn(move || {
                    node_clone.register_death_notification(move || {
                        println!("Death notification {i} fired (fixed implementation)");
                    });
                })
            })
            .collect();

        node.release();

        for h in handles {
            h.join().unwrap();
        }
    }
}