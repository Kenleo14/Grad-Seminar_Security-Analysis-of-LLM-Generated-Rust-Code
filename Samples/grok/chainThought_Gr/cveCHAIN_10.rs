use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// User-provided death recipient. Must be thread-safe and 'static.
pub trait DeathRecipient: Send + Sync + 'static {
    fn notify_death(&self);
}

/// Intrusive node that lives inside the death-notification list.
struct DeathNode {
    next: Option<NonNull<DeathNode>>,
    prev: Option<NonNull<DeathNode>>,
    recipient: Box<dyn DeathRecipient>,
}

/// Intrusive doubly-linked list of death notifications.
struct DeathList {
    head: Option<NonNull<DeathNode>>,
}

impl DeathList {
    fn new() -> Self {
        Self { head: None }
    }

    /// Add a new death notification (intrusive insert at front).
    fn add(&mut self, recipient: Box<dyn DeathRecipient>) {
        let mut node = Box::new(DeathNode {
            next: None,
            prev: None,
            recipient,
        });
        let node_ptr = NonNull::new(Box::leak(node)).expect("Box::leak never returns null");

        unsafe {
            let node_mut = node_ptr.as_mut();
            node_mut.next = self.head;
            node_mut.prev = None;

            if let Some(mut old_head) = self.head {
                old_head.as_mut().prev = Some(node_ptr);
            }
            self.head = Some(node_ptr);
        }
    }

    /// Drain the entire list and notify **while the mutex guard is still held**.
    /// This is the CVE fix: the synchronization invariant is maintained for the
    /// *entire* lifecycle of the list transfer / notification phase.
    fn drain_notify(&mut self) {
        let mut head = self.head;
        self.head = None; // list is now empty; no other thread can see these nodes

        while let Some(ptr) = head {
            unsafe {
                // Read next *before* taking ownership.
                let next = ptr.as_ref().next;

                // Reclaim ownership that was leaked in `add`.
                let node_box = Box::from_raw(ptr.as_ptr());

                // Notification happens here, still under the original lock.
                // No race window exists for any concurrent `remove` operations.
                node_box.recipient.notify_death();

                head = next;
            }
        }
    }
}

/// Internal state protected by Arc + Mutex + Atomic.
struct InnerNode {
    death_notifications: Mutex<DeathList>,
    released: AtomicBool,
}

impl InnerNode {
    fn new() -> Self {
        Self {
            death_notifications: Mutex::new(DeathList::new()),
            released: AtomicBool::new(false),
        }
    }
}

/// Public BinderNode handle.
pub struct BinderNode {
    inner: Arc<InnerNode>,
}

impl BinderNode {
    /// Create a new Binder node.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(InnerNode::new()),
        }
    }

    /// Register a death notification. Thread-safe.
    pub fn register_death_notification(&self, recipient: Box<dyn DeathRecipient>) {
        let mut list = self.inner.death_notifications.lock().unwrap();
        list.add(recipient);
    }

    /// Release the node (final reference count drops to zero).
    /// **Fixed version**: notifications are performed *while holding the lock*.
    /// This closes the race window that caused CVE-2025-68260 and upholds
    /// Rust aliasing rules for all unsafe intrusive-list operations.
    pub fn release(&self) {
        // Only the first thread performs cleanup.
        if self.inner.released.compare_exchange(
            false,
            true,
            Ordering::AcqRel,
            Ordering::Acquire,
        ).is_err() {
            return;
        }

        // Critical section now encompasses the *entire* drain + notification.
        // The mutex guard lives until after all recipients have been notified.
        let mut list_guard = self.inner.death_notifications.lock().unwrap();
        list_guard.drain_notify();
        // Lock is dropped here *after* notifications complete.
    }
}

// Auto-release on last Arc drop is deliberately omitted; callers should invoke
// `release` explicitly when the refcount reaches zero (matching real Binder semantics).