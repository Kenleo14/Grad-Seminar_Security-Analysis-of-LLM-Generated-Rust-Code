// gemini_cveZERO_2.rs

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use spin::Mutex;
use intrusive_collections::{intrusive_adapter, LinkedList, LinkedListLink};

/// A simplified death notification structure.
/// The `link` is stored intrusively, allowing the notification to exist 
/// on exactly one list at a time without additional heap allocations during moves.
pub struct DeathNotification {
    pub pid: u32,
    pub link: LinkedListLink,
}

impl DeathNotification {
    pub fn new(pid: u32) -> Self {
        Self {
            pid,
            link: LinkedListLink::new(),
        }
    }
}

// Internal adapter to map the intrusive link to the Boxed struct.
intrusive_adapter!(NotificationAdapter = Box<DeathNotification>: DeathNotification { link });

/// BinderNode management system.
/// Focuses on memory stability during high-concurrency releases.
pub struct BinderNode {
    node_id: u64,
    // Reference count to track active handles/links to this node.
    ref_count: AtomicUsize,
    // Guarded intrusive list of recipients to be notified on node death.
    death_recipients: Mutex<LinkedList<NotificationAdapter>>,
}

impl BinderNode {
    /// Creates a new BinderNode with an initial reference count of 1.
    pub fn new(node_id: u64) -> Arc<Self> {
        Arc::new(Self {
            node_id,
            ref_count: AtomicUsize::new(1),
            death_recipients: Mutex::new(LinkedList::new()),
        })
    }

    /// Registers a recipient. High concurrency is handled via the internal Mutex.
    pub fn link_to_death(&self, recipient: Box<DeathNotification>) {
        let mut recipients = self.death_recipients.lock();
        recipients.push_back(recipient);
    }

    /// Decrements the reference count and performs cleanup if the count hits zero.
    /// This uses a 'move-to-temporary' strategy to ensure the Mutex is not held 
    /// during the actual execution of notifications, preventing deadlocks.
    pub fn dec_ref(&self) {
        if self.ref_count.fetch_sub(1, Ordering::AcqRel) == 1 {
            // Memory fence to ensure subsequent reads see the latest data.
            std::sync::atomic::fence(Ordering::Acquire);
            self.process_release();
        }
    }

    /// The core cleanup logic for high-concurrency environments.
    fn process_release(&self) {
        // Create a local list on the stack to hold notifications.
        let mut local_cleanup_list = LinkedList::new(NotificationAdapter::new());

        {
            // Acquire the lock for the shortest time possible.
            let mut recipients = self.death_recipients.lock();
            // Intrusively move all elements to the local list. O(1) operation.
            local_cleanup_list.take_all(&mut recipients);
        } // Lock is dropped here.

        // Process notifications outside of the critical section.
        // This ensures that if a notification triggers another release or 
        // complex logic, the BinderNode's internal lock is already free.
        while let Some(recipient) = local_cleanup_list.pop_front() {
            Self::send_death_notification(self.node_id, recipient);
        }
    }

    fn send_death_notification(node_id: u64, recipient: Box<DeathNotification>) {
        println!(
            "[Kernel] Node {} released. Notifying PID {}.",
            node_id, recipient.pid
        );
        // recipient is dropped here.
    }
}

/// Helper to simulate a handle release.
pub fn release_handle(node: Arc<BinderNode>) {
    node.dec_ref();
}

fn main() {
    let node = BinderNode::new(0xDEADBEEF);

    // Simulate multiple subscribers
    for i in 1..=5 {
        node.link_to_death(Box::new(DeathNotification::new(1000 + i)));
    }

    println!("Releasing Binder Node...");
    // In a real system, the last Arc drop or explicit dec_ref triggers cleanup.
    node.dec_ref();
}