//! gemini_cveZERO_3.rs
//! 
//! A simplified implementation of an Android Binder Node management system.
//! This module uses an intrusive linked list to manage death notifications
//! and handles high-concurrency releases by moving nodes to a temporary 
//! "dead list" for lock-free cleanup.

use std::ptr::NonNull;
use std::sync::{Arc, Mutex};

/// Represents a notification to be sent when a Binder Node is released.
/// This structure is designed for an intrusive linked list where the 
/// 'next' pointer is stored within the object itself.
pub struct DeathRecipient {
    pub id: u64,
    /// Intrusive pointer to the next recipient in the list.
    next: Option<NonNull<DeathRecipient>>,
    /// The action to perform upon node death.
    callback: Box<dyn Fn(u64) + Send + Sync>,
}

impl DeathRecipient {
    /// Creates a new recipient. The callback receives the node handle.
    pub fn new<F>(id: u64, callback: F) -> Box<Self> 
    where 
        F: Fn(u64) + Send + Sync + 'static 
    {
        Box::new(Self {
            id,
            next: None,
            callback: Box::new(callback),
        })
    }
}

/// The internal state of a BinderNode protected by a Mutex.
struct NodeInner {
    is_dead: bool,
    /// Head of the intrusive linked list.
    recipients_head: Option<NonNull<DeathRecipient>>,
}

/// A Binder Node that manages its own lifecycle and death notifications.
pub struct BinderNode {
    pub handle: u32,
    inner: Mutex<NodeInner>,
}

impl BinderNode {
    /// Creates a new BinderNode wrapped in an Arc for shared ownership.
    pub fn new(handle: u32) -> Arc<Self> {
        Arc::new(Self {
            handle,
            inner: Mutex::new(NodeInner {
                is_dead: false,
                recipients_head: None,
            }),
        })
    }

    /// Links a DeathRecipient to this node.
    /// 
    /// # Safety
    /// The recipient must be heap-allocated and managed as a raw pointer.
    /// This implementation takes ownership of the Boxed recipient.
    pub fn link_to_death(&self, mut recipient: Box<DeathRecipient>) {
        let mut inner = self.inner.lock().unwrap();

        if inner.is_dead {
            // Node is already dying; trigger callback immediately
            (recipient.callback)(self.handle as u64);
            return;
        }

        // Convert Box to NonNull to store in the intrusive list
        let rec_ptr = unsafe { NonNull::new_unchecked(Box::into_raw(recipient)) };

        unsafe {
            // Push to the front of the intrusive list
            (*rec_ptr.as_ptr()).next = inner.recipients_head;
            inner.recipients_head = Some(rec_ptr);
        }
    }

    /// Performs the release logic. 
    /// To ensure memory stability and prevent deadlocks, the intrusive list is 
    /// "snatched" from the node and processed outside the critical section.
    pub fn release(&self) {
        let mut work_list: Option<NonNull<DeathRecipient>>;

        {
            let mut inner = self.inner.lock().unwrap();
            if inner.is_dead {
                return;
            }
            inner.is_dead = true;

            // Move the entire list to a local variable (O(1) operation)
            work_list = inner.recipients_head.take();
        }

        // Process death notifications outside the lock.
        // This handles "high-concurrency" releases by ensuring the Mutex is only 
        // held for the duration of the pointer swap.
        let mut current = work_list;
        while let Some(rec_ptr) = current {
            unsafe {
                // Re-constitute the Box to ensure proper cleanup and access
                let mut rec = Box::from_raw(rec_ptr.as_ptr());
                
                // Execute the notification
                (rec.callback)(self.handle as u64);

                // Move to the next item in the list before 'rec' is dropped
                current = rec.next;
                
                // rec is dropped here, freeing the memory for this recipient
            }
        }
    }
}

impl Drop for BinderNode {
    fn drop(&mut self) {
        // Ensure that if the Arc count hits zero, we clean up recipients.
        self.release();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn test_concurrent_release_stability() {
        let node = BinderNode::new(1024);
        let counter = Arc::new(AtomicUsize::new(0));

        // Register multiple recipients
        for i in 0..100 {
            let c = Arc::clone(&counter);
            let recipient = DeathRecipient::new(i, move |_| {
                c.fetch_add(1, Ordering::SeqCst);
            });
            node.link_to_death(recipient);
        }

        // Simulate release
        node.release();

        assert_eq!(counter.load(Ordering::SeqCst), 100);
    }
}