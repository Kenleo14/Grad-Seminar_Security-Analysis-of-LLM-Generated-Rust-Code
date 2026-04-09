// gemini_cveZERO_4.rs

use std::ptr;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};

/// Represents a Binder Node in the system.
/// The 'next' pointer is stored intrusively to avoid extra allocations 
/// and ensure pointer stability for death notifications.
pub struct BinderNode {
    pub node_id: u64,
    pub active: AtomicBool,
    /// The intrusive link. Using a Mutex here ensures that even if multiple 
    /// threads try to manipulate the list, the pointers remain valid.
    next_death_node: Mutex<*mut BinderNode>,
}

unsafe impl Send for BinderNode {}
unsafe impl Sync for BinderNode {}

impl BinderNode {
    pub fn new(node_id: u64) -> Arc<Self> {
        Arc::new(Self {
            node_id,
            active: AtomicBool::new(true),
            next_death_node: Mutex::new(ptr::null_mut()),
        })
    }
}

/// Manages the registration and bulk-cleanup of death notifications.
pub struct DeathNotificationManager {
    /// Head of the intrusive linked list.
    head: Mutex<*mut BinderNode>,
}

impl DeathNotificationManager {
    pub fn new() -> Self {
        Self {
            head: Mutex::new(ptr::null_mut()),
        }
    }

    /// Register a node to be notified upon "death" (release).
    /// Uses a Push-Front strategy on the intrusive list.
    pub fn register(&self, node: &Arc<BinderNode>) {
        let mut head_ptr = self.head.lock().expect("Manager lock poisoned");
        let mut node_next = node.next_death_node.lock().expect("Node lock poisoned");

        unsafe {
            // New node points to current head
            *node_next = *head_ptr;
            // Head now points to the new node
            *head_ptr = Arc::as_ptr(node) as *mut BinderNode;
        }
    }

    /// High-concurrency release mechanism. 
    /// This moves the entire list to a temporary local list to process
    /// cleanup without holding the global manager lock.
    pub fn release_all(&self) {
        let mut processing_list: *mut BinderNode;

        // --- Critical Section ---
        // Swap the head with null immediately to "detach" the current list.
        {
            let mut head_ptr = self.head.lock().expect("Manager lock poisoned");
            processing_list = *head_ptr;
            *head_ptr = ptr::null_mut();
        } 
        // Lock is released here. Other threads can now call register() 
        // without waiting for the cleanup of previous nodes.

        // --- Cleanup Phase ---
        while !processing_list.is_null() {
            unsafe {
                let current = &*processing_list;

                // Simulate death notification logic
                current.active.store(false, Ordering::SeqCst);
                #[cfg(debug_assertions)]
                println!("Death notification sent for Node: {}", current.node_id);

                // Safely extract the next pointer and move forward
                let mut next_ptr_lock = current.next_death_node.lock().expect("Node lock poisoned");
                let next = *next_ptr_lock;
                
                // Clear the intrusive link to prevent dangling references
                *next_ptr_lock = ptr::null_mut();
                
                processing_list = next;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn test_concurrent_death_management() {
        let manager = Arc::new(DeathNotificationManager::new());
        let mut handles = vec![];

        // Spawn multiple threads to register nodes simultaneously
        for i in 0..100 {
            let mgr = Arc::clone(&manager);
            handles.push(thread::spawn(move || {
                let node = BinderNode::new(i);
                mgr.register(&node);
            }));
        }

        for h in handles { h.join().unwrap(); }

        // Trigger the concurrent release
        manager.release_all();
    }
}