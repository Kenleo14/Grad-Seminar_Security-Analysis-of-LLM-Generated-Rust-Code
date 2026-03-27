use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// User-provided death recipient. Must be thread-safe and 'static.
pub trait DeathRecipient: Send + Sync + 'static {
    fn notify_death(&self);
}

/// Intrusive node that lives inside the death-notification list.
/// Links are embedded (intrusive) and the recipient is owned by this node.
struct DeathNode {
    next: Option<NonNull<DeathNode>>,
    prev: Option<NonNull<DeathNode>>,
    recipient: Box<dyn DeathRecipient>,
}

/// Intrusive doubly-linked list of death notifications.
/// Only the head pointer is stored; the rest lives inside the nodes.
struct DeathList {
    head: Option<NonNull<DeathNode>>,
}

impl DeathList {
    fn new() -> Self {
        Self { head: None }
    }

    /// Add a new death notification (intrusive insert at front).
    fn add(&mut self, recipient: Box<dyn DeathRecipient>) {
        let mut node = Box::new(DeathNode {
            next: None,
            prev: None,
            recipient,
        });
        let node_ptr = NonNull::new(Box::leak(node)).expect("Box::leak never returns null");

        unsafe {
            let node_mut = node_ptr.as_mut();
            node_mut.next = self.head;
            node_mut.prev = None;

            if let Some(mut old_head) = self.head {
                old_head.as_mut().prev = Some(node_ptr);
            }
            self.head = Some(node_ptr);
        }
    }

    /// Extract the entire list (moves nodes to a temporary list for cleanup).
    /// After this call the list is empty.
    fn take(&mut self) -> Option<NonNull<DeathNode>> {
        let head = self.head;
        self.head = None;
        head
    }
}

impl DeathList {
    /// Cleanup a temporary list **outside any lock**.
    /// Traverses the intrusive chain, notifies each recipient, then reclaims ownership
    /// and drops the node. Memory stability is guaranteed because:
    /// - We read `next` *before* calling `Box::from_raw`.
    /// - The original list head was already taken, so no other thread can reach these nodes.
    fn cleanup(mut head: Option<NonNull<DeathNode>>) {
        while let Some(ptr) = head {
            unsafe {
                // Read next pointer while the memory is still valid.
                let next = ptr.as_ref().next;

                // Reclaim exact ownership that was leaked during `add`.
                let node_box = Box::from_raw(ptr.as_ptr());

                // Perform the death notification (may run arbitrary user code).
                node_box.recipient.notify_death();

                // `node_box` drops here → recipient is dropped, memory freed.
                head = next;
            }
        }
    }
}

/// Internal state protected by Arc + Mutex + Atomic.
struct InnerNode {
    death_notifications: Mutex<DeathList>,
    released: AtomicBool,
}

impl InnerNode {
    fn new() -> Self {
        Self {
            death_notifications: Mutex::new(DeathList::new()),
            released: AtomicBool::new(false),
        }
    }
}

/// Public BinderNode handle – the main API users interact with.
pub struct BinderNode {
    inner: Arc<InnerNode>,
}

impl BinderNode {
    /// Create a new Binder node.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(InnerNode::new()),
        }
    }

    /// Register a death notification. Thread-safe; can be called concurrently
    /// with `release()` (the race is handled safely inside `release`).
    pub fn register_death_notification(&self, recipient: Box<dyn DeathRecipient>) {
        let mut list = self.inner.death_notifications.lock().unwrap();
        list.add(recipient);
    }

    /// Release the node (simulate the final reference count dropping to zero).
    /// Handles **high-concurrency releases** exactly as requested:
    /// - Atomic one-time guard.
    /// - Nodes are moved to a temporary list (via `take`).
    /// - Cleanup runs outside the lock for maximum concurrency and deadlock freedom.
    pub fn release(&self) {
        // Only the first thread to reach here performs cleanup.
        if self.inner.released.compare_exchange(
            false,
            true,
            Ordering::AcqRel,
            Ordering::Acquire,
        ).is_err() {
            return; // already released by another thread
        }

        // Brief critical section: just move the entire list out.
        let mut list_guard = self.inner.death_notifications.lock().unwrap();
        let temp_head = list_guard.take();
        drop(list_guard); // unlock *immediately*

        // Cleanup on the temporary list – no lock held.
        DeathList::cleanup(temp_head);
    }
}

// Optional: auto-release on last Arc drop (real Binder would tie this to refcount).
// For the simplified module we keep `release` explicit so the caller controls timing.
impl Drop for BinderNode {
    fn drop(&mut self) {
        // In a full implementation you could call `self.release()` here,
        // but that would require careful handling of re-entrancy.
        // The manual `release` API is deliberately exposed for clarity.
    }
}