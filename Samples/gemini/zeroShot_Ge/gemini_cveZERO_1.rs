use std::ptr;
use std::sync::{Arc, Mutex};
use std::thread;

/// An intrusive node for death notifications.
/// In a real Binder system, this would likely be part of a larger 
/// 'BinderRef' or 'Handle' object.
pub struct DeathNotification {
    pub pid: u32,
    // The intrusive link. Using a raw pointer for simplicity in this 
    // demonstration, though Pin<T> is preferred for production.
    next: *mut DeathNotification,
}

impl DeathNotification {
    pub fn new(pid: u32) -> *mut Self {
        Box::into_raw(Box::new(Self {
            pid,
            next: ptr::null_mut(),
        }))
    }
}

/// Manages Binder nodes and their associated death recipients.
pub struct BinderNode {
    pub node_id: u64,
    /// The head of the intrusive linked list. 
    /// Protected by a Mutex to handle concurrent registrations/releases.
    death_list: Mutex<*mut DeathNotification>,
}

impl BinderNode {
    pub fn new(node_id: u64) -> Arc<Self> {
        Arc::new(Self {
            node_id,
            death_list: Mutex::new(ptr::null_mut()),
        })
    }

    /// Links a death notification to this node.
    /// Memory Stability: The notification is leaked into a raw pointer 
    /// to ensure it lives as long as the list needs it.
    pub fn link_to_death(&self, notification: *mut DeathNotification) {
        let mut head = self.death_list.lock().unwrap();
        unsafe {
            (*notification).next = *head;
            *head = notification;
        }
    }

    /// High-Concurrency Release:
    /// Moves the entire list to a temporary local pointer to allow 
    /// the lock to be released immediately.
    pub fn release_notifications(&self) {
        let mut local_cleanup_head: *mut DeathNotification = ptr::null_mut();

        // Critical Section: Minimal duration
        {
            let mut head = self.death_list.lock().unwrap();
            local_cleanup_head = *head;
            *head = ptr::null_mut(); 
        }

        // Cleanup: Performed outside the lock. 
        // Other threads can now register new notifications or 
        // trigger their own releases without waiting for this loop.
        unsafe {
            self.process_cleanup(local_cleanup_head);
        }
    }

    unsafe fn process_cleanup(&self, mut current: *mut DeathNotification) {
        while !current.is_null() {
            let next = (*current).next;
            
            // Log or execute death callback logic
            println!("[Node {}] Cleaning notification for PID: {}", self.node_id, (*current).pid);
            
            // Re-claim memory to drop it properly
            let _ = Box::from_raw(current);
            current = next;
        }
    }
}

fn main() {
    let node = BinderNode::new(1024);

    // Simulate concurrent registrations
    let mut handles = vec![];
    for i in 0..5 {
        let node_ref = Arc::clone(&node);
        handles.push(thread::spawn(move || {
            let dn = DeathNotification::new(1000 + i);
            node_ref.link_to_death(dn);
        }));
    }

    for h in handles { h.join().unwrap(); }

    // Concurrent release
    println!("Starting node release...");
    node.release_notifications();
    println!("Release complete.");
}