use std::ptr::NonNull;
use std::sync::Mutex;

/// An intrusive node for death notifications.
/// The 'next' and 'prev' pointers live inside the struct itself.
pub struct DeathNotification {
    pub id: u64,
    next: Option<NonNull<DeathNotification>>,
    prev: Option<NonNull<DeathNotification>>,
}

impl DeathNotification {
    pub fn new(id: u64) -> Self {
        Self {
            id,
            next: None,
            prev: None,
        }
    }
}

/// A simplified Binder Node management system.
pub struct BinderNode {
    pub node_id: u32,
    /// The head of the intrusive linked list, protected by a Mutex.
    /// This allows multiple threads to safely register death notifications.
    notifications: Mutex<Option<NonNull<DeathNotification>>>,
}

impl BinderNode {
    pub fn new(node_id: u32) -> Self {
        Self {
            node_id,
            notifications: Mutex::new(None),
        }
    }

    /// Links a notification to the node's death list.
    /// Uses an intrusive approach: the notification is boxed and then "leaked"
    /// into the raw pointer list to ensure stability during concurrent access.
    pub fn link_to_death(&self, notification: Box<DeathNotification>) {
        let mut guard = self.notifications.lock().unwrap();
        let node_ptr = unsafe { NonNull::new_unchecked(Box::into_raw(notification)) };

        unsafe {
            if let Some(mut head) = *guard {
                (*node_ptr.as_ptr()).next = Some(head);
                (*head.as_ptr()).prev = Some(node_ptr);
            }
            *guard = Some(node_ptr);
        }
    }

    /// High-concurrency cleanup.
    /// Moves the entire list to a local "batch" for destruction.
    /// This minimizes the time the Mutex is held, preventing lock contention
    /// while notifications are being processed/dropped.
    pub fn release_notifications(&self) -> Vec<Box<DeathNotification>> {
        let mut collected = Vec::new();

        // 1. Critical Section: Extract the list head and clear the node's list.
        let mut current_ptr = {
            let mut guard = self.notifications.lock().unwrap();
            guard.take()
        };

        // 2. Non-Critical Section: Reclaim memory. 
        // We are now safe from concurrent modifications to this specific list.
        while let Some(mut node) = current_ptr {
            unsafe {
                let next = node.as_ref().next;
                // Re-box the raw pointer to ensure the Drop trait is called.
                collected.push(Box::from_raw(node.as_ptr()));
                current_ptr = next;
            }
        }

        collected
    }
}

// Safety: The Mutex ensures internal pointers are accessed thread-safely.
unsafe impl Send for BinderNode {}
unsafe impl Sync for BinderNode {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;

    #[test]
    fn test_concurrent_cleanup() {
        let node = Arc::new(BinderNode::new(1));
        let mut threads = vec![];

        // Spawn 10 threads to register notifications
        for i in 0..10 {
            let n_clone = Arc::clone(&node);
            threads.push(thread::spawn(move || {
                let dn = Box::new(DeathNotification::new(i));
                n_clone.link_to_death(dn);
            }));
        }

        for t in threads { t.join().unwrap(); }

        // Cleanup
        let released = node.release_notifications();
        assert_eq!(released.len(), 10);
    }
}