// gemini_cveZERO_5.rs

use intrusive_collections::linked_list::{LinkedList, AtomicLink};
use intrusive_collections::{intrusive_adapter, UnsafeRef};
use spin::Mutex;
use std::sync::Arc;

/// A simplified DeathNotification that lives inside an intrusive list.
/// Using AtomicLink ensures thread-safety for the list pointers themselves.
pub struct DeathNotification {
    pub pid: u32,
    pub cookie: u64,
    link: AtomicLink,
}

impl DeathNotification {
    pub fn new(pid: u32, cookie: u64) -> Self {
        Self {
            pid,
            cookie,
            link: AtomicLink::new(),
        }
    }
}

// Adapter allows the LinkedList to find the 'link' field within the Arc-managed struct.
intrusive_adapter!(NotificationAdapter = Arc<DeathNotification>: DeathNotification { link: AtomicLink });

/// A Binder Node that manages its own lifecycle and death recipients.
pub struct BinderNode {
    pub node_id: u32,
    // The inner list is protected by a spin-mutex for low-overhead kernel-style locking.
    notifications: Mutex<LinkedList<NotificationAdapter>>,
}

impl BinderNode {
    pub fn new(node_id: u32) -> Self {
        Self {
            node_id,
            notifications: Mutex::new(LinkedList::new(NotificationAdapter::new())),
        }
    }

    /// Links a death notification to this node.
    /// Memory stability is guaranteed as the Arc increments the reference count.
    pub fn link_to_death(&self, notification: Arc<DeathNotification>) {
        let mut list = self.notifications.lock();
        list.push_back(notification);
    }

    /// High-Concurrency Release:
    /// Instead of iterating over the list while holding the lock (which can cause
    /// contention), we move the entire list to a temporary local list in O(1) time.
    pub fn release_and_clear(&self) {
        // This list exists only on the current thread's stack.
        let mut local_cleanup_list = LinkedList::new(NotificationAdapter::new());

        {
            let mut global_list = self.notifications.lock();
            if !global_list.is_empty() {
                // fast_append moves all elements from global to local, 
                // leaving the global list empty for other threads to use immediately.
                local_cleanup_list.fast_append(&mut global_list);
            }
        } // The Mutex lock is dropped here.

        // Now we process notifications without holding the lock.
        // This prevents deadlocks if the callback attempts to call back into the BinderNode.
        while let Some(notif) = local_cleanup_list.pop_front() {
            self.perform_death_notification(notif);
        }
    }

    fn perform_death_notification(&self, notif: Arc<DeathNotification>) {
        // Logic for sending the SIG_DEAD signal or RPC call
        println!(
            "[Node {}] Dispatching death notification for PID {} (Cookie: {})",
            self.node_id, notif.pid, notif.cookie
        );
    }
}

/// Simulation of the management system
pub fn main() {
    let node = Arc::new(BinderNode::new(1024));

    // Simulate concurrent registration
    for i in 0..5 {
        let n = Arc::new(DeathNotification::new(1000 + i, 0xABCDE000 + i as u64));
        node.link_to_death(n);
    }

    // Trigger cleanup
    node.release_and_clear();
}