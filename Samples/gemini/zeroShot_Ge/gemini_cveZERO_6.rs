use std::sync::{Arc, Mutex};
use std::fmt;

/// The 'Intrusive' hook that lives inside the data structure.
/// In a real Binder implementation, this would likely use raw pointers,
/// but Arc allows us to maintain Rust's safety guarantees during concurrent cleanup.
pub struct DeathRecipient {
    pub id: u64,
    // The intrusive link to the next recipient in the list.
    next: Mutex<Option<Arc<DeathRecipient>>>,
}

impl DeathRecipient {
    pub fn new(id: u64) -> Self {
        Self {
            id,
            next: Mutex::new(None),
        }
    }
}

impl fmt::Debug for DeathRecipient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Recipient(id: {})", self.id)
    }
}

/// A simplified Android Binder Node.
pub struct BinderNode {
    pub handle: u32,
    /// The head of the intrusive linked list of death notifications.
    /// Protected by a Mutex to handle concurrent registration/unregistration.
    notification_head: Mutex<Option<Arc<DeathRecipient>>>,
}

impl BinderNode {
    pub fn new(handle: u32) -> Self {
        Self {
            handle,
            notification_head: Mutex::new(None),
        }
    }

    /// Link a death recipient to this node.
    /// This is an O(1) operation that pushes the recipient onto the head of the list.
    pub fn link_to_death(&self, recipient: Arc<DeathRecipient>) {
        let mut head_guard = self.notification_head.lock().unwrap();
        
        // Lock the recipient's internal 'next' hook to update the link
        let mut next_guard = recipient.next.lock().unwrap();
        
        // Point the new recipient to the current head
        *next_guard = head_guard.take();
        
        // Update the head to be the new recipient
        *head_guard = Some(recipient);
    }

    /// Handles a high-concurrency release scenario.
    /// Instead of processing notifications while holding the node lock, 
    /// we 'detach' the entire list to a local stack variable.
    pub fn release_node(&self) {
        let mut current_recipient: Option<Arc<DeathRecipient>>;

        // Phase 1: Atomically detach the intrusive list.
        // This minimizes lock contention for other threads trying to link/unlink.
        {
            let mut head_guard = self.notification_head.lock().unwrap();
            current_recipient = head_guard.take();
        } 

        // Phase 2: Process the detached list.
        // Memory stability is maintained because we own the Arcs in this local scope.
        while let Some(recipient) = current_recipient {
            // Logically "trigger" the death notification
            self.on_binder_died(&recipient);

            // Move to the next link in the intrusive chain
            let mut next_guard = recipient.next.lock().unwrap();
            current_recipient = next_guard.take(); 
            // Note: .take() here clears the intrusive link, effectively cleaning up memory.
        }
    }

    fn on_binder_died(&self, recipient: &DeathRecipient) {
        println!("[Node {}] Thread {:?}: Delivering death notification to Recipient {}", 
            self.handle, 
            std::thread::current().id(),
            recipient.id
        );
    }
}

// --- Example Concurrency Test ---

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn test_concurrent_death_notifications() {
        let node = Arc::new(BinderNode::new(42));
        let mut handles = Vec::new();

        // Spawn 10 threads to concurrently link death recipients
        for i in 0..10 {
            let node_ref = Arc::clone(&node);
            handles.push(thread::spawn(move || {
                let recipient = Arc::new(DeathRecipient::new(i));
                node_ref.link_to_death(recipient);
            }));
        }

        for h in handles { h.join().unwrap(); }

        // Trigger the high-concurrency release
        node.release_node();
    }
}