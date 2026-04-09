// File: binder_node.rs
// Dependencies: parking_lot = "0.12", intrusive_collections = "0.9"

use intrusive_collections::{intrusive_adapter, LinkedList, LinkedListLink};
use parking_lot::Mutex;
use std::sync::Arc;
use std::thread;

/// 1. DeathNotification: The "link" structure.
/// In Binder, when a process dies, the kernel notifies these objects.
/// We use an intrusive link so the notification carries its own list metadata.
pub struct DeathNotification {
    pub process_id: u32,
    link: LinkedListLink,
}

impl DeathNotification {
    pub fn new(pid: u32) -> Self {
        Self {
            process_id: pid,
            link: LinkedListLink::new(),
        }
    }
}

// The adapter facilitates the conversion between a pointer to the link 
// and a pointer to the container (DeathNotification).
intrusive_adapter!(NotificationAdapter = Arc<DeathNotification>: DeathNotification { link: LinkedListLink });

/// 2. BinderNode: The management entity.
/// Manages death notifications using a mutex-protected intrusive list.
pub struct BinderNode {
    pub node_id: u64,
    // Intrusive list ensures no extra heap allocations when adding to the list.
    notifications: Mutex<LinkedList<NotificationAdapter>>,
}

impl BinderNode {
    pub fn new(id: u64) -> Self {
        Self {
            node_id: id,
            notifications: Mutex::new(LinkedList::new(NotificationAdapter::new())),
        }
    }

    /// Links a death notification to this node. 
    /// Thread-safe and O(1) insertion.
    pub fn link_to_death(&self, notification: Arc<DeathNotification>) {
        let mut list = self.notifications.lock();
        list.push_back(notification);
    }

    /// Release mechanism designed for high concurrency.
    /// It "swaps" the list out to a local variable to perform cleanup 
    /// without holding the lock, preventing blocking and deadlocks.
    pub fn release(&self) {
        // Create a temporary, thread-local list.
        let mut local_cleanup_list = LinkedList::new(NotificationAdapter::new());

        {
            // Lock scope: minimize the critical section.
            let mut list = self.notifications.lock();
            
            // Atomically move all elements from the shared list to our local list.
            // This is a constant-time operation in intrusive lists.
            local_cleanup_list.fast_clear_and_take(&mut *list);
        } 

        // The lock is now released. We can iterate and trigger callbacks 
        // without worrying about other threads being blocked on link_to_death.
        while let Some(notify) = local_cleanup_list.pop_front() {
            self.trigger_death_callback(notify.process_id);
        }
    }

    fn trigger_death_callback(&self, pid: u32) {
        println!("[Node {}] Notification: Process {} has died.", self.node_id, pid);
    }
}

/// 3. Usage Example / Test Harness
fn main() {
    let node = Arc::new(BinderNode::new(0xDEADBEEF));
    let mut handles = vec![];

    // Simulate concurrent registration from multiple threads
    for i in 0..10 {
        let node_ref = Arc::clone(&node);
        let handle = thread::spawn(move || {
            let notification = Arc::new(DeathNotification::new(1000 + i));
            node_ref.link_to_death(notification);
        });
        handles.push(handle);
    }

    for h in handles { h.join().unwrap(); }

    println!("All notifications registered. Initiating concurrent release...");

    // Trigger release
    node.release();
    
    println!("Release complete. Memory stable.");
}