use std::sync::{Arc, Mutex, Weak};
use std::ptr::NonNull;
use std::mem;

/// Intrusive links embedded inside each node (mimicking rust_binder style).
#[repr(C)]
#[derive(Debug)]
struct ListLinks {
    next: Option<NonNull<DeathNode>>,
    prev: Option<NonNull<DeathNode>>,
}

impl ListLinks {
    const fn new() -> Self {
        ListLinks { next: None, prev: None }
    }
}

/// Death notification node with intrusive links.
struct DeathNode {
    links: ListLinks,
    recipient: Box<dyn Fn(Arc<BinderNode>) + Send + Sync>,
    node: Weak<BinderNode>,
    generation: u64,      // Generation when inserted (for race detection)
    detached: bool,       // Set when moved to temp list or removed
    is_dead: bool,
}

impl DeathNode {
    fn new(
        recipient: Box<dyn Fn(Arc<BinderNode>) + Send + Sync>,
        node: Weak<BinderNode>,
        generation: u64,
    ) -> Self {
        DeathNode {
            links: ListLinks::new(),
            recipient,
            node,
            generation,
            detached: false,
            is_dead: false,
        }
    }
}

/// The Binder-like node owning the death list.
pub struct BinderNode {
    inner: Mutex<BinderNodeInner>,
}

struct BinderNodeInner {
    death_list: Option<NonNull<DeathNode>>, // Head of intrusive list
    generation: u64,                        // Incremented on drain
    is_released: bool,
}

impl BinderNode {
    pub fn new() -> Arc<Self> {
        Arc::new(BinderNode {
            inner: Mutex::new(BinderNodeInner {
                death_list: None,
                generation: 0,
                is_released: false,
            }),
        })
    }

    /// Register a death notification (insert at head under lock).
    pub fn register_death_notification(
        self: &Arc<Self>,
        recipient: Box<dyn Fn(Arc<BinderNode>) + Send + Sync>,
    ) -> bool {
        let weak_self = Arc::downgrade(self);
        let mut guard = self.inner.lock().unwrap();

        if guard.is_released {
            let node_clone = self.clone();
            drop(guard);
            (recipient)(node_clone);
            return false;
        }

        let gen = guard.generation;
        let mut new_node = Box::new(DeathNode::new(recipient, weak_self, gen));
        let node_ptr = NonNull::from(Box::leak(new_node));

        unsafe {
            if let Some(head) = guard.death_list {
                (*head.as_ptr()).links.prev = Some(node_ptr);
            }
            (*node_ptr.as_ptr()).links.next = guard.death_list;
            (*node_ptr.as_ptr()).links.prev = None;
            guard.death_list = Some(node_ptr);
        }
        true
    }

    /// Safe concurrent remove (unregister). Checks generation to avoid touching moved nodes.
    pub fn unregister_death_notification(
        &self,
        recipient_ptr: *const dyn Fn(Arc<BinderNode>) + Send + Sync,
    ) -> bool {
        let mut guard = self.inner.lock().unwrap();
        if guard.is_released {
            return false;
        }

        let current_gen = guard.generation;
        let mut current_opt = guard.death_list;

        while let Some(curr_ptr) = current_opt {
            let curr = unsafe { &mut *curr_ptr.as_ptr() };

            // Generation mismatch or already detached → node was moved to temp list, skip safely
            if curr.generation != current_gen || curr.detached {
                current_opt = curr.links.next;
                continue;
            }

            if std::ptr::eq(
                &*curr.recipient as *const _ as *const (),
                recipient_ptr as *const (),
            ) {
                // Generation matches → safe to unlink under lock
                unsafe {
                    Self::unlink_node(&mut guard.death_list, curr_ptr);
                }
                curr.detached = true;
                unsafe { drop(Box::from_raw(curr_ptr.as_ptr())); }
                return true;
            }
            current_opt = curr.links.next;
        }
        false
    }

    /// Release: Move entire list to local stack temp list (short critical section),
    /// then process callbacks outside the lock. Uses generation + detached flag
    /// to prevent concurrent remove() from corrupting pointers.
    pub fn release(self: Arc<Self>) {
        let mut temp_head: Option<NonNull<DeathNode>> = None;
        let mut guard = self.inner.lock().unwrap();

        if guard.is_released {
            return;
        }
        guard.is_released = true;

        // Phase 1: Atomic move to temp + increment generation (minimal lock time)
        std::mem::swap(&mut temp_head, &mut guard.death_list);
        guard.generation = guard.generation.wrapping_add(1);

        drop(guard); // Release lock early — this is the performance win

        // Phase 2: Process temp list outside lock (callbacks may be slow)
        let mut current = temp_head;
        while let Some(mut curr_ptr) = current {
            let curr = unsafe { &mut *curr_ptr.as_ptr() };

            // Mark as detached (prevents any late remove from touching it)
            curr.detached = true;

            if !curr.is_dead {
                if let Some(node_arc) = curr.node.upgrade() {
                    curr.is_dead = true;
                    (curr.recipient)(node_arc);
                }
            }

            current = curr.links.next;

            // Deallocate after callback
            unsafe {
                drop(Box::from_raw(curr_ptr.as_ptr()));
            }
        }
    }

    /// Helper: Unlink under lock (only called when generation matches).
    unsafe fn unlink_node(head: &mut Option<NonNull<DeathNode>>, node: NonNull<DeathNode>) {
        let n = &mut *node.as_ptr();
        if let Some(prev) = n.links.prev {
            (*prev.as_ptr()).links.next = n.links.next;
        } else {
            *head = n.links.next;
        }
        if let Some(next) = n.links.next {
            (*next.as_ptr()).links.prev = n.links.prev;
        }
        n.links.next = None;
        n.links.prev = None;
    }
}

// Comprehensive tests covering concurrency and the CVE avoidance
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Barrier;

    #[test]
    fn test_release_with_concurrent_unregister() {
        let node = BinderNode::new();
        let call_count = Arc::new(AtomicUsize::new(0));
        let barrier = Arc::new(Barrier::new(2));

        // Register
        let count_clone = call_count.clone();
        let barrier_clone = barrier.clone();
        node.register_death_notification(Box::new(move |_n| {
            count_clone.fetch_add(1, Ordering::SeqCst);
            barrier_clone.wait();
        }));

        let node_clone = node.clone();
        let handle = std::thread::spawn(move || {
            // Concurrent unregister attempt (may race with release)
            node_clone.unregister_death_notification(
                &*Box::new(|_| {}) as *const dyn Fn(Arc<BinderNode>) + Send + Sync,
            );
        });

        node.release(); // Should succeed without corruption
        barrier.wait(); // Ensure callback ran if registered

        handle.join().unwrap();
        assert_eq!(call_count.load(Ordering::SeqCst), 1); // Callback fires exactly once
    }

    #[test]
    fn test_multiple_releases_and_registers() {
        let node = BinderNode::new();
        let count = Arc::new(AtomicUsize::new(0));

        for _ in 0..5 {
            let c = count.clone();
            node.register_death_notification(Box::new(move |_n| {
                c.fetch_add(1, Ordering::SeqCst);
            }));
        }

        let node_clone = node.clone();
        let h = std::thread::spawn(move || node_clone.release());

        // Simulate late register after release starts
        std::thread::sleep(std::time::Duration::from_millis(1));
        let post = node.register_death_notification(Box::new(|_n| {}));
        assert!(!post); // Should invoke immediately if released

        h.join().unwrap();
        assert_eq!(count.load(Ordering::SeqCst), 5);
    }
}