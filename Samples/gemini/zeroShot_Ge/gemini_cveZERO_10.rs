use std::ptr;
use std::sync::atomic::{AtomicPtr, Ordering};
use std::sync::Mutex;

/// A simplified death notification. In a real system, this would
/// likely contain a trait object or a callback function.
pub struct DeathRecipient {
    /// The intrusive pointer to the next recipient in the chain.
    next: AtomicPtr<DeathRecipient>,
    /// Unique identifier for the process/service to be notified.
    pub id: u64,
}

impl DeathRecipient {
    pub fn new(id: u64) -> Self {
        Self {
            next: AtomicPtr::new(ptr::null_mut()),
            id,
        }
    }
}

/// The Binder Node manages a list of recipients that must be 
/// notified when the node "dies" or is released.
pub struct BinderNode {
    /// Head of the intrusive linked list.
    notification_head: AtomicPtr<DeathRecipient>,
    /// Ensures structural integrity during 'link' operations.
    list_lock: Mutex<()>,
}

// Explicitly mark as thread-safe since we are managing raw pointers.
unsafe impl Send for BinderNode {}
unsafe impl Sync for BinderNode {}

impl BinderNode {
    pub fn new() -> Self {
        Self {
            notification_head: AtomicPtr::new(ptr::null_mut()),
            list_lock: Mutex::new(()),
        }
    }

    /// Register a new death notification. 
    /// This takes ownership of the Boxed recipient via raw pointer.
    pub fn link_to_death(&self, recipient: Box<DeathRecipient>) {
        let recipient_ptr = Box::into_raw(recipient);
        
        // We lock only to prevent race conditions between multiple writers.
        let _guard = self.list_lock.lock().unwrap();
        
        let current_head = self.notification_head.load(Ordering::Relaxed);
        unsafe {
            (*recipient_ptr).next.store(current_head, Ordering::Relaxed);
        }
        
        // Release ordering ensures the writes to 'next' are visible 
        // to any thread performing an Acquire swap.
        self.notification_head.store(recipient_ptr, Ordering::Release);
    }

    /// Detaches the entire list of notifications and returns them.
    /// This is the "High-Concurrency" path: it clears the node 
    /// instantly so the node can be dropped or reused while 
    /// the notifications are being processed elsewhere.
    pub fn release_notifications(&self) -> Vec<u64> {
        // Atomic swap detaches the entire list in one operation.
        let mut current_ptr = self.notification_head.swap(ptr::null_mut(), Ordering::Acquire);
        
        let mut ids = Vec::new();

        // Iterate through the detached "temporary" list.
        while !current_ptr.is_null() {
            unsafe {
                // Re-claim ownership of the pointer to safely drop memory.
                let recipient = Box::from_raw(current_ptr);
                ids.push(recipient.id);
                
                // Move to next before the current 'recipient' Box goes out of scope.
                current_ptr = recipient.next.load(Ordering::Relaxed);
            }
        }
        
        ids
    }
}

/// Ensure we don't leak memory if the node is dropped before release.
impl Drop for BinderNode {
    fn drop(&mut self) {
        self.release_notifications();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::sync::Arc;

    #[test]
    fn test_concurrent_release() {
        let node = Arc::new(BinderNode::new());
        let mut handles = vec![];

        // Add 100 notifications
        for i in 0..100 {
            let n = node.clone();
            handles.push(thread::spawn(move || {
                n.link_to_death(Box::new(DeathRecipient::new(i)));
            }));
        }

        for h in handles { h.join().unwrap(); }

        let processed = node.release_notifications();
        assert_eq!(processed.len(), 100);
        
        // Second call should return empty as list was moved.
        assert_eq!(node.release_notifications().len(), 0);
    }
}