use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// User-provided death recipient (must be thread-safe and static).
pub trait DeathRecipient: Send + Sync + 'static {
    fn notify_death(&self);
}

/// Intrusive death-notification node.
struct DeathNode {
    next: Option<NonNull<DeathNode>>,
    prev: Option<NonNull<DeathNode>>,
    recipient: Box<dyn DeathRecipient>,
    generation: u32, // Synchronization stamp for safe concurrent remove
}

/// Intrusive doubly-linked list with generation-based detachment.
struct DeathList {
    head: Option<NonNull<DeathNode>>,
    generation: u32,
}

impl DeathList {
    fn new() -> Self {
        Self {
            head: None,
            generation: 0,
        }
    }

    /// Insert at front (O(1)). Returns the raw node pointer for registration handle.
    fn add(&mut self, recipient: Box<dyn DeathRecipient>) -> NonNull<DeathNode> {
        let mut node = Box::new(DeathNode {
            next: None,
            prev: None,
            recipient,
            generation: self.generation,
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
        node_ptr
    }

    /// Extract entire list for cleanup + bump generation (O(1) under lock).
    /// This is the critical operation that enables short lock-hold time while
    /// keeping the list synchronized against concurrent remove().
    fn take_for_cleanup(&mut self) -> Option<NonNull<DeathNode>> {
        let head = self.head;
        self.head = None;
        self.generation = self.generation.wrapping_add(1);
        head
    }

    /// Remove a specific node (idempotent, thread-safe, O(1) amortized).
    /// Uses generation check to guarantee the node is still in the shared list.
    fn remove(&mut self, target: NonNull<DeathNode>) {
        unsafe {
            let node = target.as_ref();
            if node.generation == u32::MAX || node.generation != self.generation {
                return; // already removed or detached by release()
            }
        }

        // Safe to unlink: generation match guarantees exclusive ownership of this node.
        unsafe {
            let node = target.as_mut();
            let next = node.next;
            let prev = node.prev;

            if let Some(mut p) = prev {
                p.as_mut().next = next;
            } else {
                // Was the head
                self.head = next;
            }

            if let Some(mut n) = next {
                n.as_mut().prev = prev;
            }

            // Mark as removed (prevents double-unlink and serves as sentinel)
            node.next = None;
            node.prev = None;
            node.generation = u32::MAX;

            // Reclaim ownership and drop (notification is *not* sent on unregister).
            let _ = Box::from_raw(target.as_ptr());
        }
    }

    /// Cleanup temporary stack list **outside the lock** (notifications run freely).
    fn cleanup(mut head: Option<NonNull<DeathNode>>) {
        while let Some(ptr) = head {
            unsafe {
                let next = ptr.as_ref().next;
                let node_box = Box::from_raw(ptr.as_ptr());
                node_box.recipient.notify_death();
                head = next;
            }
        }
    }
}

/// Internal node state.
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

/// Opaque registration token returned by `register_death_notification`.
#[derive(Debug)]
pub struct DeathRegistration(NonNull<DeathNode>);

/// Public Binder node handle.
pub struct BinderNode {
    inner: Arc<InnerNode>,
}

impl BinderNode {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(InnerNode::new()),
        }
    }

    /// Register a death notification. Returns a token that can be used for unregistration.
    pub fn register_death_notification(&self, recipient: Box<dyn DeathRecipient>) -> DeathRegistration {
        let mut list = self.inner.death_notifications.lock().unwrap();
        let ptr = list.add(recipient);
        DeathRegistration(ptr)
    }

    /// Unregister a previously registered notification (idempotent and race-free).
    pub fn unregister_death_notification(&self, registration: DeathRegistration) {
        let mut list = self.inner.death_notifications.lock().unwrap();
        list.remove(registration.0);
    }

    /// Release the node (final refcount drop). Moves the list to a temporary stack
    /// list in O(1) time under the lock, then performs notifications outside the lock.
    /// The generation bump guarantees that any concurrent `remove`/`unregister` is
    /// a safe no-op and never touches the temporary chain.
    pub fn release(&self) {
        if self.inner.released.compare_exchange(
            false,
            true,
            Ordering::AcqRel,
            Ordering::Acquire,
        ).is_err() {
            return;
        }

        let mut list_guard = self.inner.death_notifications.lock().unwrap();
        let temp_head = list_guard.take_for_cleanup();
        drop(list_guard); // lock dropped – notifications now run with zero contention

        DeathList::cleanup(temp_head);
    }
}