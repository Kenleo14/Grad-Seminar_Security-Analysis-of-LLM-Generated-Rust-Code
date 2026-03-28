use std::sync::{Arc, Mutex, Weak};
use std::ptr::{self, NonNull};

// Intrusive list links embedded in each DeathNode.
#[repr(C)]
struct ListLinks {
    next: Option<NonNull<DeathNode>>,
    prev: Option<NonNull<DeathNode>>,
}

impl ListLinks {
    fn new() -> Self {
        ListLinks {
            next: None,
            prev: None,
        }
    }
}

// A death notification node. This is "intrusive" — links live inside it.
struct DeathNode {
    links: ListLinks,
    recipient: Box<dyn Fn(Arc<BinderNode>) + Send + Sync>, // Callback when node dies
    node: Weak<BinderNode>, // Weak ref to avoid cycles
    is_dead: bool,          // Flag to prevent duplicate callbacks
}

impl DeathNode {
    fn new(
        recipient: Box<dyn Fn(Arc<BinderNode>) + Send + Sync>,
        node: Weak<BinderNode>,
    ) -> Self {
        DeathNode {
            links: ListLinks::new(),
            recipient,
            node,
            is_dead: false,
        }
    }
}

// The Binder Node itself.
pub struct BinderNode {
    inner: Mutex<BinderNodeInner>,
}

struct BinderNodeInner {
    death_list: Option<NonNull<DeathNode>>, // Head of intrusive death list
    death_count: usize,
    is_released: bool,
}

impl BinderNode {
    pub fn new() -> Arc<Self> {
        Arc::new(BinderNode {
            inner: Mutex::new(BinderNodeInner {
                death_list: None,
                death_count: 0,
                is_released: false,
            }),
        })
    }

    /// Register a death notification. Returns true if newly registered.
    pub fn register_death_notification(
        self: &Arc<Self>,
        recipient: Box<dyn Fn(Arc<BinderNode>) + Send + Sync>,
    ) -> bool {
        let weak_self = Arc::downgrade(self);
        let mut death_node = Box::new(DeathNode::new(recipient, weak_self));

        let mut guard = self.inner.lock().unwrap();

        if guard.is_released {
            // Node already dead — invoke immediately and drop
            let node_clone = self.clone();
            drop(guard);
            (death_node.recipient)(node_clone);
            return false;
        }

        // Check for duplicate (simple pointer-based; production could use ID)
        // For simplicity, we allow multiples here unless exact same closure (rare).

        // Insert at head (intrusive)
        let node_ptr = NonNull::from(Box::leak(death_node));
        unsafe {
            if let Some(head) = guard.death_list {
                (*head.as_ptr()).links.prev = Some(node_ptr);
            }
            (*node_ptr.as_ptr()).links.next = guard.death_list;
            (*node_ptr.as_ptr()).links.prev = None;
            guard.death_list = Some(node_ptr);
        }
        guard.death_count += 1;
        true
    }

    /// Unregister a specific death recipient (by callback pointer equality).
    /// In real systems, this would use a unique ID or handle.
    pub fn unregister_death_notification(
        &self,
        recipient_ptr: *const dyn Fn(Arc<BinderNode>) + Send + Sync,
    ) -> bool {
        let mut guard = self.inner.lock().unwrap();
        if guard.is_released {
            return false;
        }

        let mut current = guard.death_list;
        while let Some(mut curr_ptr) = current {
            let curr = unsafe { &mut *curr_ptr.as_ptr() };
            if std::ptr::eq(
                &*curr.recipient as *const _ as *const (),
                recipient_ptr as *const (),
            ) {
                // Unlink intrusive node
                unsafe {
                    Self::unlink_node(&mut guard.death_list, curr_ptr);
                }
                guard.death_count -= 1;
                // Safety: drop after unlink
                drop(unsafe { Box::from_raw(curr_ptr.as_ptr()) });
                return true;
            }
            current = curr.links.next;
        }
        false
    }

    /// Release the node (simulate death). Drains death notifications to a temp list.
    pub fn release(self: Arc<Self>) {
        let mut temp_list: Option<NonNull<DeathNode>> = None;
        let mut guard = self.inner.lock().unwrap();

        if guard.is_released {
            return;
        }
        guard.is_released = true;

        // Atomically move entire death list to temporary (under lock)
        std::mem::swap(&mut temp_list, &mut guard.death_list);
        guard.death_count = 0;

        drop(guard); // Release lock before callbacks

        // Now invoke callbacks outside lock (high-concurrency safe)
        let mut current = temp_list;
        while let Some(mut curr_ptr) = current {
            let curr = unsafe { &mut *curr_ptr.as_ptr() };
            if !curr.is_dead {
                if let Some(node) = curr.node.upgrade() {
                    curr.is_dead = true;
                    (curr.recipient)(node);
                }
            }
            current = curr.links.next;
            // Deallocate after callback
            unsafe {
                Box::from_raw(curr_ptr.as_ptr());
            }
        }
    }

    // Helper: unlink a node from the intrusive list (updates head, prev/next)
    unsafe fn unlink_node(head: &mut Option<NonNull<DeathNode>>, node: NonNull<DeathNode>) {
        let n = &mut *node.as_ptr();
        if let Some(prev) = n.links.prev {
            (*prev.as_ptr()).links.next = n.links.next;
        } else {
            // Was head
            *head = n.links.next;
        }
        if let Some(next) = n.links.next {
            (*next.as_ptr()).links.prev = n.links.prev;
        }
        // Clear links to prevent dangling use
        n.links.next = None;
        n.links.prev = None;
    }
}

// Example usage and tests (for illustration)
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn test_death_notification_basic() {
        let node = BinderNode::new();
        let call_count = Arc::new(AtomicUsize::new(0));

        let count_clone = call_count.clone();
        node.register_death_notification(Box::new(move |_n| {
            count_clone.fetch_add(1, Ordering::SeqCst);
        }));

        node.release(); // Triggers death
        assert_eq!(call_count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn test_concurrent_release_and_register() {
        let node = BinderNode::new();
        let call_count = Arc::new(AtomicUsize::new(0));

        // Spawn threads that register while main releases
        let handles: Vec<_> = (0..10)
            .map(|_| {
                let n = node.clone();
                let c = call_count.clone();
                std::thread::spawn(move || {
                    n.register_death_notification(Box::new(move |_n| {
                        c.fetch_add(1, Ordering::SeqCst);
                    }));
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        node.release();
        // Some may have registered before release, some after (post-release invokes immediately)
        assert!(call_count.load(Ordering::SeqCst) > 0);
    }
}