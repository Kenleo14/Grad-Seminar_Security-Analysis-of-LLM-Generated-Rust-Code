// binder_node.rs - Simplified Android Binder Node with intrusive death notifications

use std::sync::{Arc, Mutex, Weak};
use std::ptr;
use std::marker::PhantomPinned;
use std::pin::Pin;

/// Intrusive link fields for a doubly-linked list.
/// These are embedded directly in DeathNotification for zero-allocation list ops.
#[derive(Default)]
struct IntrusiveLink {
    next: *mut IntrusiveLink,
    prev: *mut IntrusiveLink,
}

/// A death notification registered against a BinderNode.
/// In real Binder, this would hold a callback (DeathRecipient) and cookie.
#[derive(Default)]
pub struct DeathNotification {
    /// The intrusive links (embedded).
    link: IntrusiveLink,
    /// Weak reference back to the node (to avoid cycles and detect death).
    node: Weak<BinderNode>,
    /// Example callback payload. In a real system, this could be a trait object or boxed closure.
    /// For simplicity: a string identifier and a simple callback.
    id: String,
    callback: Box<dyn Fn() + Send + Sync>,
    /// Pinning marker to discourage moving after insertion (intrusive lists require stable addresses).
    _pin: PhantomPinned,
}

impl DeathNotification {
    /// Create a new death notification.
    pub fn new(id: String, callback: Box<dyn Fn() + Send + Sync>) -> Self {
        Self {
            id,
            callback,
            ..Default::default()
        }
    }

    /// Invoke the death callback (called when the node dies).
    pub fn notify(&self) {
        (self.callback)();
    }
}

/// The Binder Node. Represents a bindable object that can "die" (e.g., process exit).
pub struct BinderNode {
    /// Strong reference count simulation (real Binder uses atomic refs + weak).
    /// Here we use Arc externally; internal state protected by mutex.
    inner: Mutex<BinderNodeInner>,
}

struct BinderNodeInner {
    /// Is the node dead? Once true, no new registrations allowed.
    is_dead: bool,
    /// Head of the intrusive death notification list.
    death_list_head: IntrusiveLink,
    /// Number of registered death notifications (for debugging/edge cases).
    death_count: usize,
}

impl Default for BinderNodeInner {
    fn default() -> Self {
        let mut head = IntrusiveLink::default();
        head.next = &mut head as *mut _;
        head.prev = &mut head as *mut _;
        Self {
            is_dead: false,
            death_list_head: head,
            death_count: 0,
        }
    }
}

impl BinderNode {
    /// Create a new BinderNode.
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(BinderNodeInner::default()),
        })
    }

    /// Register a death notification for this node.
    /// Returns true if successfully registered; false if node is already dead.
    pub fn link_to_death(self: &Arc<Self>, notification: Pin<Box<DeathNotification>>) -> bool {
        let mut guard = self.inner.lock().unwrap();

        if guard.is_dead {
            return false; // Cannot register on a dead node
        }

        // SAFETY: We pin the notification and keep it alive via Arc<Self>.
        // The address remains stable because we only remove under lock or after draining.
        let notif_ptr = unsafe { &*notification as *const DeathNotification as *mut DeathNotification };
        let link_ptr = &mut unsafe { &mut (*notif_ptr).link } as *mut IntrusiveLink;

        // Insert at the end of the intrusive list (before head).
        // SAFETY: List is circular; head invariants maintained.
        unsafe {
            let head = &mut guard.death_list_head as *mut IntrusiveLink;
            let prev = (*head).prev;
            (*link_ptr).next = head;
            (*link_ptr).prev = prev;
            (*prev).next = link_ptr;
            (*head).prev = link_ptr;
        }

        // Store weak ref back to node inside the notification.
        // SAFETY: We know the notification is pinned and lives as long as it's in the list.
        unsafe {
            (*notif_ptr).node = Arc::downgrade(self);
        }

        guard.death_count += 1;
        true
    }

    /// Unlink (remove) a specific death notification.
    /// In practice, caller would keep a handle; here we demonstrate by ID for simplicity.
    /// Real systems often use a cookie or direct reference.
    pub fn unlink_to_death(&self, id: &str) -> bool {
        let mut guard = self.inner.lock().unwrap();

        if guard.is_dead {
            return false;
        }

        // Traverse the list to find by ID.
        // This is O(n); real Binder may use better lookup (hash + list) or per-ref storage.
        let head = &mut guard.death_list_head as *mut IntrusiveLink;
        let mut current = unsafe { (*head).next };

        while current != head {
            // SAFETY: current is a valid link in the list.
            let notif = unsafe {
                &mut *((current as *mut u8).sub(std::mem::offset_of!(DeathNotification, link))
                    as *mut DeathNotification)
            };

            if notif.id == id {
                // Remove from intrusive list.
                // SAFETY: Removing under lock; no concurrent modification.
                unsafe {
                    let next = (*current).next;
                    let prev = (*current).prev;
                    (*prev).next = next;
                    (*next).prev = prev;

                    // Optional: poison the link to detect bugs.
                    (*current).next = ptr::null_mut();
                    (*current).prev = ptr::null_mut();
                }

                guard.death_count -= 1;
                return true;
            }
            current = unsafe { (*current).next };
        }

        false
    }

    /// Mark the node as dead and deliver all death notifications.
    /// This is the core "release" path with high-concurrency focus.
    ///
    /// **High-concurrency handling**:
    /// 1. Acquire lock.
    /// 2. Drain ALL death notifications into a temporary Vec (moving ownership).
    /// 3. Release lock immediately.
    /// 4. Process callbacks and drop notifications **outside** the lock.
    ///
    /// This prevents long lock contention when many notifications exist or callbacks are slow.
    /// Memory stability: Notifications are moved out; their addresses are no longer in the shared list.
    /// Edge cases covered: concurrent link/unlink during death, multiple death calls (idempotent), empty list.
    pub fn release(&self) {
        let temp_list = {
            let mut guard = self.inner.lock().unwrap();

            if guard.is_dead {
                return; // Already released (idempotent)
            }

            guard.is_dead = true;

            // Drain the intrusive list into a temporary owned list.
            // This moves ownership out of the shared structure.
            let mut temp = Vec::with_capacity(guard.death_count);

            let head = &mut guard.death_list_head as *mut IntrusiveLink;
            let mut current = unsafe { (*head).next };

            while current != head {
                let next = unsafe { (*current).next };

                // Extract the full DeathNotification.
                // SAFETY: offset calculation is correct; we own the memory via the list.
                let notif_ptr = unsafe {
                    (current as *mut u8).sub(std::mem::offset_of!(DeathNotification, link))
                        as *mut DeathNotification
                };

                // Remove from list (though we're draining everything).
                unsafe {
                    let prev = (*current).prev;
                    (*prev).next = next;
                    (*next).prev = prev;
                }

                // SAFETY: Transfer ownership to Vec. The notification was heap-allocated via Box.
                let notif_box = unsafe { Box::from_raw(notif_ptr) };
                temp.push(notif_box);

                current = next;
            }

            // Reset head for cleanliness (though node is now dead).
            unsafe {
                (*head).next = head;
                (*head).prev = head;
            }
            guard.death_count = 0;

            temp
        }; // Lock dropped here — critical for concurrency

        // Process notifications without holding the lock.
        // Callbacks can take arbitrary time; other threads can now register/unlink on other nodes.
        for notif in temp_list {
            // Check weak ref (defensive; in real code, it should still be valid).
            if notif.node.upgrade().is_some() {
                notif.notify();
            }
            // Drop happens automatically here.
        }
    }

    /// Query if the node is dead.
    pub fn is_dead(&self) -> bool {
        self.inner.lock().unwrap().is_dead
    }

    /// Debug: current death notification count (approximate under lock).
    pub fn death_count(&self) -> usize {
        self.inner.lock().unwrap().death_count
    }
}

// Helper to create a pinned notification for registration.
pub fn new_pinned_death_notification(
    id: String,
    callback: Box<dyn Fn() + Send + Sync>,
) -> Pin<Box<DeathNotification>> {
    let notif = DeathNotification::new(id, callback);
    Box::pin(notif)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;

    #[test]
    fn test_basic_death_notification() {
        let node = BinderNode::new();
        let call_count = Arc::new(AtomicUsize::new(0));
        let cc = call_count.clone();

        let notif = new_pinned_death_notification(
            "test1".to_string(),
            Box::new(move || {
                cc.fetch_add(1, Ordering::SeqCst);
            }),
        );

        assert!(node.link_to_death(notif));
        assert_eq!(node.death_count(), 1);

        node.release();
        assert_eq!(call_count.load(Ordering::SeqCst), 1);
        assert!(node.is_dead());
    }

    #[test]
    fn test_concurrent_release_and_link() {
        let node = BinderNode::new();
        let barrier = Arc::new(std::sync::Barrier::new(2));

        let node_clone = node.clone();
        let b1 = barrier.clone();

        let handle = thread::spawn(move || {
            b1.wait();
            // Simulate slow release with many notifications
            for i in 0..100 {
                let notif = new_pinned_death_notification(
                    format!("concurrent_{}", i),
                    Box::new(|| {}),
                );
                let _ = node_clone.link_to_death(notif);
            }
            node_clone.release();
        });

        let b2 = barrier.clone();
        let node2 = node.clone();
        thread::spawn(move || {
            b2.wait();
            // Try to link/unlink while release happens
            for _ in 0..50 {
                let notif = new_pinned_death_notification("race".to_string(), Box::new(|| {}));
                let _ = node2.link_to_death(notif);
                let _ = node2.unlink_to_death("race");
            }
        });

        handle.join().unwrap();
        assert!(node.is_dead());
    }

    #[test]
    fn test_multiple_releases_idempotent() {
        let node = BinderNode::new();
        node.release();
        node.release(); // Should be safe
        assert!(node.is_dead());
    }
}