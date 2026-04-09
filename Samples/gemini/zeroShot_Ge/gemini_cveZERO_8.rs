//! binder_node.rs - Simplified Android Binder Node Management
//! 
//! Focuses on memory stability and high-concurrency cleanup using
//! an intrusive-style linked list for death notifications.

use std::sync::{Arc, Mutex};
use std::ptr::NonNull;

/// Represents a registration for a death event.
/// In an intrusive system, the "node" structure contains the link metadata.
pub struct DeathNotification {
    pub pid: u32,
    /// Pointer to the next notification in the list.
    /// Using NonNull for memory safety and covariance.
    next: Option<NonNull<DeathNotification>>,
}

impl DeathNotification {
    pub fn new(pid: u32) -> Self {
        Self { pid, next: None }
    }
}

/// A Binder Node representing a local or remote object.
pub struct BinderNode {
    pub handle: u64,
    /// The head of the intrusive linked list. 
    /// Mutex ensures thread-safe registration and atomic "taking" of the list.
    notifications_head: Mutex<Option<NonNull<DeathNotification>>>,
}

impl BinderNode {
    /// Creates a new BinderNode wrapped in an Arc for shared ownership.
    pub fn new(handle: u64) -> Arc<Self> {
        Arc::new(Self {
            handle,
            notifications_head: Mutex::new(None),
        })
    }

    /// Registers a death notification. 
    /// This "intrudes" into the list by taking ownership of the Box.
    pub fn register_notification(&self, pid: u32) {
        let mut note = Box::new(DeathNotification::new(pid));
        
        let mut guard = self.notifications_head.lock().unwrap();
        
        // Link the current head to our new notification's next pointer
        note.next = *guard;
        
        // Move the new notification to the head of the list
        let raw_ptr = NonNull::new(Box::into_raw(note)).expect("Box should not be null");
        *guard = Some(raw_ptr);
    }

    /// High-Concurrency Release Logic.
    /// 
    /// This method performs a "Stage and Clear" operation. It swaps the list head
    /// with None under the lock, then processes the memory deallocation outside 
    /// the lock to prevent blocking other threads.
    pub fn release_notifications(&self) {
        let mut current_head: Option<NonNull<DeathNotification>> = None;

        // --- Critical Section Start ---
        {
            let mut guard = self.notifications_head.lock().unwrap();
            // Atomically steal the entire list
            current_head = guard.take();
        } 
        // --- Critical Section End ---

        // Process the cleanup outside the lock. 
        // This ensures memory stability even if many nodes are being released at once.
        let mut next_node = current_head;
        while let Some(mut node_ptr) = next_node {
            unsafe {
                // Convert raw pointer back to Box to trigger drop/cleanup
                let mut boxed_node = Box::from_raw(node_ptr.as_ptr());
                
                println!(
                    "Node {}: Cleaning up death notification for PID {}", 
                    self.handle, boxed_node.pid
                );
                
                // Set the next pointer for the next iteration before Box is dropped
                next_node = boxed_node.next;
                
                // boxed_node drops here naturally
            }
        }
    }
}

// Ensure memory is cleaned up if the node itself is dropped
impl Drop for BinderNode {
    fn drop(&mut self) {
        self.release_notifications();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn test_concurrent_registration_and_release() {
        let node = BinderNode::new(1024);
        let mut handles = vec![];

        // Spawn threads to register notifications concurrently
        for i in 0..10 {
            let node_clone = Arc::clone(&node);
            handles.push(thread::spawn(move || {
                node_clone.register_notification(1000 + i);
            }));
        }

        for h in handles { h.join().unwrap(); }

        // Clean up
        node.release_notifications();
    }
}